//! Preserve no-fill styled runs without removing their text/layout content.
use super::*;

pub(super) fn content(node: &SceneNode, line: &str, line_start: usize) -> String {
    if !node.fill_disabled && node.text_runs.iter().all(|run| !run.fill_disabled) {
        return xml_escape(line);
    }
    let mut out = String::new();
    let mut cursor = 0;
    for run in &node.text_runs {
        let start = run.start.saturating_sub(line_start).min(line.len());
        let end = run.end.saturating_sub(line_start).min(line.len());
        if end <= start
            || start < cursor
            || !line.is_char_boundary(start)
            || !line.is_char_boundary(end)
        {
            continue;
        }
        out.push_str(&xml_escape(&line[cursor..start]));
        if run.fill_disabled {
            let _ = write!(
                out,
                r#"<tspan fill="none">{}</tspan>"#,
                xml_escape(&line[start..end])
            );
        } else if let Some(color) = run.fill {
            let _ = write!(
                out,
                r#"<tspan fill="{}" fill-opacity="{}">{}</tspan>"#,
                color_to_rgb(color),
                color.a,
                xml_escape(&line[start..end])
            );
        } else {
            out.push_str(&xml_escape(&line[start..end]));
        }
        cursor = end;
    }
    out.push_str(&xml_escape(&line[cursor..]));
    out
}
