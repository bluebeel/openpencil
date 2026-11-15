//! Applying the viewBox-aware root scale to element attributes,
//! path `d` data and point lists.

use super::*;

/// Multiply every numeric coord on the element by `scale` so a
/// 24-unit viewBox shape renders at the same final size as the TS
/// app's viewBox-aware path. For `<path>` we tokenise the `d`
/// string + scale every coord; for shape elements we scale
/// `width` / `height` / `x` / `y` / `r` / `cx` / `cy` / `rx` /
/// `ry` / `x1` / `y1` / `x2` / `y2` in place.
pub(super) fn apply_scale_to_attrs(el: &mut SvgElement, scale: f64) {
    if (scale - 1.0).abs() < 1e-6 {
        return;
    }
    if el.tag == "path" {
        if let Some(pos) = el.attrs.iter().position(|(k, _)| k == "d") {
            let scaled = scale_svg_path(&el.attrs[pos].1, scale);
            el.attrs[pos].1 = scaled;
        }
        return;
    }
    if el.tag == "polyline" || el.tag == "polygon" {
        if let Some(pos) = el.attrs.iter().position(|(k, _)| k == "points") {
            let scaled = scale_svg_points(&el.attrs[pos].1, scale);
            el.attrs[pos].1 = scaled;
        }
        return;
    }
    let scalable: &[&str] = &[
        "x", "y", "width", "height", "r", "rx", "ry", "cx", "cy", "x1", "y1", "x2", "y2",
    ];
    for (k, v) in &mut el.attrs {
        if !scalable.iter().any(|s| *s == k) {
            continue;
        }
        if let Ok(n) = v.trim().parse::<f64>() {
            *v = format!("{}", n * scale);
        }
    }
}

/// Use the same grammar reader as bounds/editable conversion, including the
/// one-character SVG arc flags and implicit command repetition.
fn scale_svg_path(d: &str, scale: f64) -> String {
    crate::svg_path_data::scale_svg_path(d, scale).unwrap_or_default()
}

/// Scale a `points="x1,y1 x2,y2 …"` list for `<polyline>` /
/// `<polygon>`. Returns the same separator style (`x,y x,y …`).
fn scale_svg_points(s: &str, scale: f64) -> String {
    parse_point_list(s)
        .into_iter()
        .map(|(x, y)| format!("{},{}", x * scale, y * scale))
        .collect::<Vec<_>>()
        .join(" ")
}
