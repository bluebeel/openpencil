//! One reusable vector text definition per painted line slice. Invisible
//! siblings still participate in SVG text layout; no estimated prefix widths.
use super::*;
use std::fmt::{self, Write};

pub(super) struct Limited(pub String);
impl Write for Limited {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        if self
            .0
            .len()
            .checked_add(value.len())
            .is_none_or(|n| n > MAX_GEOMETRY_BYTES)
        {
            return Err(fmt::Error);
        }
        self.0.push_str(value);
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub(super) struct Slice {
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub run: Option<usize>,
}
pub(super) fn slices(n: &SceneNode) -> Result<Vec<Slice>, SvgExportError> {
    let text = n.text.as_deref().unwrap_or("");
    if !n.font_size.is_finite()
        || n.text_runs.iter().any(|r| {
            !r.font_size.is_finite()
                || r.start > r.end
                || r.end > text.len()
                || !text.is_char_boundary(r.start)
                || !text.is_char_boundary(r.end)
        })
    {
        return Err(invalid(n));
    }
    let mut result = Vec::new();
    let mut line_start = 0;
    for (line, content) in text.split('\n').enumerate() {
        let line_end = line_start + content.len();
        let mut pos = line_start;
        while pos < line_end {
            if result.len() >= 4096 {
                return Err(limit(n));
            }
            let run = n
                .text_runs
                .iter()
                .position(|r| r.start <= pos && pos < r.end);
            let end = run
                .map(|i| n.text_runs[i].end.min(line_end))
                .unwrap_or_else(|| {
                    n.text_runs
                        .iter()
                        .map(|r| r.start)
                        .filter(|s| *s > pos)
                        .min()
                        .unwrap_or(line_end)
                        .min(line_end)
                });
            result.push(Slice {
                line,
                start: pos,
                end,
                run,
            });
            pos = end;
        }
        line_start = line_end.saturating_add(1);
    }
    Ok(result)
}

pub(super) fn features(n: &SceneNode) -> Result<String, SvgExportError> {
    let mut settings = Limited(String::new());
    for (index, (tag, value)) in n.font_features.iter().enumerate() {
        if index != 0 {
            settings.write_str(",").map_err(|_| limit(n))?;
        }
        settings.write_char('"').map_err(|_| limit(n))?;
        for ch in tag.chars() {
            if ch == '"' || ch == '\\' {
                settings.write_char('\\').map_err(|_| limit(n))?;
            }
            if ch.is_control() {
                write!(settings, "\\{:x} ", ch as u32).map_err(|_| limit(n))?;
            } else {
                settings.write_char(ch).map_err(|_| limit(n))?;
            }
        }
        write!(settings, "\" {value}").map_err(|_| limit(n))?;
    }
    Ok(settings.0)
}

pub(super) fn definition(
    out: &mut impl Write,
    n: &SceneNode,
    slices: &[Slice],
    selected: usize,
    id: &str,
    features: &str,
) -> fmt::Result {
    let text = n.text.as_deref().unwrap_or("");
    let target = slices[selected];
    let r = normalize_rect(n.bounds);
    let size = if n.font_size > 0.0 {
        n.font_size
    } else {
        TEXT_DEFAULT_FONT_SIZE
    };
    // Preserve the existing SVG serializer's baseline/line coordinate policy.
    // Font/spacing attributes remain authored rather than fitting textLength.
    let baseline = r.origin.y + size + 1.0 + target.line as f32 * size * 1.35;
    write!(
        out,
        r#"<text id="{id}" x="{}" y="{baseline}" font-family="{}" font-size="{size}" letter-spacing="{}" xml:space="preserve" fill="currentColor" stroke="none""#,
        r.origin.x,
        xml_escape(if n.font_family.is_empty() {
            "system-ui, sans-serif"
        } else {
            &n.font_family
        }),
        n.letter_spacing
    )?;
    if n.font_weight > 0 {
        write!(out, r#" font-weight="{}""#, n.font_weight)?;
    }
    if n.italic {
        out.write_str(r#" font-style="italic""#)?;
    }
    if let Some(kerning) = n.font_kerning {
        write!(
            out,
            r#" font-kerning="{}""#,
            if kerning { "normal" } else { "none" }
        )?;
    }
    if !features.is_empty() {
        write!(
            out,
            r#" style="font-feature-settings:{}""#,
            xml_escape(features)
        )?;
    }
    out.write_char('>')?;
    for (index, slice) in slices
        .iter()
        .enumerate()
        .filter(|(_, s)| s.line == target.line)
    {
        let run = slice.run.map(|i| &n.text_runs[i]);
        write!(
            out,
            r#"<tspan visibility="{}""#,
            if index == selected {
                "visible"
            } else {
                "hidden"
            }
        )?;
        if let Some(run) = run {
            if run.font_size > 0.0 {
                write!(out, r#" font-size="{}""#, run.font_size)?;
            }
            if run.font_weight > 0 {
                write!(out, r#" font-weight="{}""#, run.font_weight)?;
            }
            write!(
                out,
                r#" font-style="{}""#,
                if run.italic { "italic" } else { "normal" }
            )?;
        }
        let underline = run.map_or(n.underline, |r| r.underline);
        let strike = run.map_or(n.strikethrough, |r| r.strikethrough);
        let decoration = match (underline, strike) {
            (true, true) => "underline line-through",
            (true, false) => "underline",
            (false, true) => "line-through",
            _ => "none",
        };
        write!(out, r#" text-decoration="{decoration}""#)?;
        if let Some(skip) = run
            .and_then(|r| r.text_decoration_skip_ink)
            .or(n.text_decoration_skip_ink)
        {
            write!(
                out,
                r#" style="text-decoration-skip-ink:{}""#,
                if skip { "auto" } else { "none" }
            )?;
        }
        write!(
            out,
            ">{}</tspan>",
            xml_escape(&text[slice.start..slice.end])
        )?;
    }
    out.write_str("</text>")
}
