//! SVG/CSS RGB colors with float opacity, without packing through RGBA8.

pub(super) fn rgba(raw: &str) -> Option<[f64; 4]> {
    let value = raw.trim().to_ascii_lowercase();
    if value == "transparent" {
        return Some([0.0; 4]);
    }
    if let Some(inner) = value
        .strip_prefix("rgba(")
        .or_else(|| value.strip_prefix("rgb("))
        .and_then(|s| s.strip_suffix(')'))
    {
        let parts: Vec<&str> = inner
            .split(|c: char| c == ',' || c == '/' || c.is_ascii_whitespace())
            .filter(|p| !p.is_empty())
            .collect();
        if parts.len() != 3 && parts.len() != 4 {
            return None;
        }
        let component = |raw: &str| {
            let (raw, denominator) = if let Some(raw) = raw.strip_suffix('%') {
                (raw, 100.0)
            } else {
                (raw, 255.0)
            };
            raw.parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .map(|v| (v / denominator).clamp(0.0, 1.0))
        };
        let alpha = if parts.len() == 4 {
            let (raw, denominator) = if let Some(raw) = parts[3].strip_suffix('%') {
                (raw, 100.0)
            } else {
                (parts[3], 1.0)
            };
            raw.parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())?
                .clamp(0.0, denominator)
                / denominator
        } else {
            1.0
        };
        if !alpha.is_finite() {
            return None;
        }
        return Some([
            component(parts[0])?,
            component(parts[1])?,
            component(parts[2])?,
            alpha,
        ]);
    }
    // Hex and the complete CSS named-color table have integer channel inputs.
    let color = value.parse::<svgtypes::Color>().ok()?;
    Some([
        f64::from(color.red) / 255.0,
        f64::from(color.green) / 255.0,
        f64::from(color.blue) / 255.0,
        f64::from(color.alpha) / 255.0,
    ])
}
pub(super) fn css([r, g, b, a]: [f64; 4]) -> String {
    format!("rgba({},{},{},{})", r * 255.0, g * 255.0, b * 255.0, a)
}

/// Keep existing opaque hex/named-color wire values stable; functional source
/// colors retain f64 components and alpha instead of an RGBA8 round trip.
pub(super) fn canonical_css(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if !raw.contains('(') {
        if let Ok(color) = raw.parse::<svgtypes::Color>() {
            return Some(if color.alpha == 255 {
                format!("#{:02x}{:02x}{:02x}", color.red, color.green, color.blue)
            } else {
                format!(
                    "#{:02x}{:02x}{:02x}{:02x}",
                    color.red, color.green, color.blue, color.alpha
                )
            });
        }
    }
    rgba(raw).map(css)
}
