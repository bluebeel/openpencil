use super::*;
use crate::image_field_geometry::{prepare_with_source, ImageFieldError};
use jian_ops_schema::style::ImageExtend;
use kurbo::{Affine, Point};
use op_util::xml_escape::escape_xml as escape;
use std::fmt::Write;
fn matrix(m: Affine) -> String {
    let [a, b, c, d, e, f] = m.as_coeffs();
    format!("matrix({a} {b} {c} {d} {e} {f})")
}
fn rectangle(out: &mut String, b: [f64; 4], attrs: &str) {
    let [x, y, w, h] = b;
    let _ = write!(
        out,
        r##"<rect x="{x}" y="{y}" width="{w}" height="{h}" {attrs}/>"##
    );
}
fn cover_in(m: Affine, b: [f64; 4]) -> [f64; 4] {
    let [x, y, w, h] = b;
    let ps = [
        Point::new(x, y),
        Point::new(x + w, y),
        Point::new(x + w, y + h),
        Point::new(x, y + h),
    ]
    .map(|p| m * p);
    [
        ps.iter().map(|p| p.x).fold(f64::INFINITY, f64::min),
        ps.iter().map(|p| p.y).fold(f64::INFINITY, f64::min),
        ps.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max),
        ps.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max),
    ]
}
fn error(source: ImageFieldError) -> ImageSvgError {
    ImageSvgError::InvalidGeometry { source }
}
pub(super) fn definition(
    id: &str,
    m: &SvgImageMetadata,
    original: &str,
    thumbnail: Option<&str>,
    edges: Option<&EdgeSources>,
) -> Result<String, ImageSvgError> {
    let metadata = encode_metadata(m)?;
    let [x, y, w, h] = m.coverage;
    let mut out = String::new();
    let _ = write!(
        out,
        r##"<pattern id="{id}" patternUnits="userSpaceOnUse" patternContentUnits="userSpaceOnUse" x="{x}" y="{y}" width="{w}" height="{h}" viewBox="{x} {y} {w} {h}" overflow="hidden" {METADATA_ATTRIBUTE}="{metadata}">"##
    );
    let _ = write!(
        out,
        r##"<defs><image id="image-source-original-{id}" width="1" height="1" href="{}" preserveAspectRatio="none"/>"##,
        escape(original)
    );
    if let Some(src) = thumbnail {
        let _ = write!(
            out,
            r##"<image id="image-source-thumbnail-{id}" width="1" height="1" href="{}" preserveAspectRatio="none"/>"##,
            escape(src)
        );
    }
    out.push_str("</defs>");
    let source = if m.display_thumbnail {
        "thumbnail"
    } else {
        "original"
    };
    if m.display_thumbnail && thumbnail.is_none() {
        return Err(ImageSvgError::InvalidMetadata {
            stage: "display source",
        });
    }
    let source_id = format!("image-source-{source}-{id}");
    let source_href = if m.display_thumbnail {
        thumbnail.unwrap()
    } else {
        original
    };
    let filter = crate::image_field_geometry::image_adjustment_matrix(
        crate::render_backend::ImageAdjustments {
            exposure: m.paint.exposure.unwrap_or(0.0) as f32,
            contrast: m.paint.contrast.unwrap_or(0.0) as f32,
            saturation: m.paint.saturation.unwrap_or(0.0) as f32,
            temperature: m.paint.temperature.unwrap_or(0.0) as f32,
            tint: m.paint.tint.unwrap_or(0.0) as f32,
            highlights: m.paint.highlights.unwrap_or(0.0) as f32,
            shadows: m.paint.shadows.unwrap_or(0.0) as f32,
        },
    );
    if let Some(values) = filter {
        let values = values
            .iter()
            .map(f32::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        let _ = write!(
            out,
            r##"<filter id="image-adjust-{id}" filterUnits="userSpaceOnUse" x="{x}" y="{y}" width="{w}" height="{h}" color-interpolation-filters="sRGB"><feColorMatrix type="matrix" values="{values}"/></filter>"##
        );
    }
    let _ = write!(
        out,
        r##"<g opacity="{}"{}>"##,
        m.render_opacity,
        if filter.is_some() {
            format!(r##" filter="url(#image-adjust-{id})""##)
        } else {
            String::new()
        }
    );
    let basis = Affine::translate((m.bounds[0], m.bounds[1]))
        * Affine::scale_non_uniform(m.bounds[2], m.bounds[3]);
    let (parent, coverage) = if let Some(pattern) = &m.geometry.pattern {
        let cell = basis * Affine::new(pattern.transform);
        if cell.determinant() == 0.0 {
            return Err(error(ImageFieldError::Singular));
        }
        let coverage = cover_in(cell.inverse(), m.coverage);
        let repeat_x = matches!(
            pattern.extend_x,
            jian_ops_schema::style::ImageRepeat::Repeat
                | jian_ops_schema::style::ImageRepeat::Reflect
        );
        let repeat_y = matches!(
            pattern.extend_y,
            jian_ops_schema::style::ImageRepeat::Repeat
                | jian_ops_schema::style::ImageRepeat::Reflect
        );
        let sx = if repeat_x { 0.0 } else { coverage[0] - 1.0 };
        let sy = if repeat_y { 0.0 } else { coverage[1] - 1.0 };
        let tw = if repeat_x {
            if pattern.extend_x == jian_ops_schema::style::ImageRepeat::Reflect {
                2.0
            } else {
                1.0
            }
        } else {
            coverage[2] - coverage[0] + 2.0
        };
        let th = if repeat_y {
            if pattern.extend_y == jian_ops_schema::style::ImageRepeat::Reflect {
                2.0
            } else {
                1.0
            }
        } else {
            coverage[3] - coverage[1] + 2.0
        };
        let _ = write!(
            out,
            r##"<defs><pattern id="image-tile-{id}" patternUnits="userSpaceOnUse" patternContentUnits="userSpaceOnUse" x="{sx}" y="{sy}" width="{tw}" height="{th}" viewBox="{sx} {sy} {tw} {th}" patternTransform="{}" overflow="{}">"##,
            matrix(cell),
            if pattern.clip { "hidden" } else { "visible" }
        );
        let mut content = String::new();
        field(
            &mut content,
            id,
            m,
            &source_id,
            source_href,
            Affine::IDENTITY,
            [sx, sy, tw, th],
            edges,
        )?;
        let _ = write!(out, r##"<defs><g id="image-cell-{id}">"##);
        if pattern.clip {
            let _ = write!(
                out,
                r##"<clipPath id="image-tile-clip-{id}"><rect width="1" height="1"/></clipPath><g clip-path="url(#image-tile-clip-{id})">{content}</g>"##
            );
        } else {
            out.push_str(&content);
        }
        out.push_str("</g></defs>");
        let _ = write!(out, r##"<use href="#image-cell-{id}"/>"##);
        if pattern.extend_x == jian_ops_schema::style::ImageRepeat::Reflect {
            let _ = write!(
                out,
                r##"<use href="#image-cell-{id}" transform="translate(2 0) scale(-1 1)"/>"##
            );
        }
        if pattern.extend_y == jian_ops_schema::style::ImageRepeat::Reflect {
            let _ = write!(
                out,
                r##"<use href="#image-cell-{id}" transform="translate(0 2) scale(1 -1)"/>"##
            );
        }
        if pattern.extend_x == jian_ops_schema::style::ImageRepeat::Reflect
            && pattern.extend_y == jian_ops_schema::style::ImageRepeat::Reflect
        {
            let _ = write!(
                out,
                r##"<use href="#image-cell-{id}" transform="translate(2 2) scale(-1 -1)"/>"##
            );
        }
        out.push_str("</pattern></defs>");
        rectangle(
            &mut out,
            m.coverage,
            &format!(r##"fill="url(#image-tile-{id})""##),
        );
        (None, coverage)
    } else {
        (Some(basis), [x, y, x + w, y + h])
    };
    if let Some(parent) = parent {
        field(
            &mut out,
            id,
            m,
            &source_id,
            source_href,
            parent,
            [
                coverage[0],
                coverage[1],
                coverage[2] - coverage[0],
                coverage[3] - coverage[1],
            ],
            edges,
        )?;
    }
    out.push_str("</g></pattern>");
    Ok(out)
}
fn field(
    out: &mut String,
    id: &str,
    m: &SvgImageMetadata,
    source_id: &str,
    source_href: &str,
    parent: Affine,
    coverage: [f64; 4],
    edges: Option<&EdgeSources>,
) -> Result<(), ImageSvgError> {
    let placement = Affine::new(m.geometry.transform);
    let viewport = m.geometry.viewport.as_deref();
    // Standard SVG handles deferred intrinsic image aspect ratios directly;
    // no source pixels or fake dimensions are needed for ordinary decal images.
    if m.geometry.extend_x == ImageExtend::Decal && m.geometry.extend_y == ImageExtend::Decal {
        if let Some(v) = viewport.filter(|_| m.dimensions.is_none()) {
            let [vw, vh] = v.size;
            let transform = parent * placement * Affine::scale_non_uniform(1.0 / vw, 1.0 / vh);
            let align = |value: f64, axis: &str| {
                format!(
                    "{axis}{}",
                    if value == 0.0 {
                        "Min"
                    } else if value == 1.0 {
                        "Max"
                    } else {
                        "Mid"
                    }
                )
            };
            let preserve = match v.fit {
                jian_ops_schema::style::ImageViewportFit::None => "none".into(),
                fit => format!(
                    "{}{} {}",
                    align(v.align[0], "x"),
                    align(v.align[1], "Y"),
                    if fit == jian_ops_schema::style::ImageViewportFit::Slice {
                        "slice"
                    } else {
                        "meet"
                    }
                ),
            };
            let _ = write!(
                out,
                r##"<g transform="{}"><svg width="{vw}" height="{vh}" overflow="{}"><image width="{vw}" height="{vh}" href="{}" preserveAspectRatio="{preserve}"/></svg></g>"##,
                matrix(transform),
                if v.clip { "hidden" } else { "visible" },
                escape(source_href)
            );
            return Ok(());
        }
    }
    let mut g = m.geometry.clone();
    g.pattern = None;
    let prepared = prepare_with_source(&g, [0.0, 0.0, 1.0, 1.0], m.dimensions.unwrap_or([0, 0]))
        .map_err(error)?;
    let source_to_parent = parent * prepared.image_to_document;
    if source_to_parent.determinant() == 0.0 {
        return Err(error(ImageFieldError::Singular));
    }
    let range = cover_in(source_to_parent.inverse(), coverage);
    let clip = viewport.is_some_and(|v| v.clip);
    if clip {
        let _ = write!(
            out,
            r##"<defs><clipPath id="image-viewport-{id}" clipPathUnits="userSpaceOnUse"><rect width="1" height="1" transform="{}"/></clipPath></defs><g clip-path="url(#image-viewport-{id})">"##,
            matrix(parent * placement)
        );
    }
    let _ = write!(out, r##"<g transform="{}">"##, matrix(source_to_parent));
    let rx = matches!(
        m.geometry.extend_x,
        ImageExtend::Repeat | ImageExtend::Reflect
    );
    let ry = matches!(
        m.geometry.extend_y,
        ImageExtend::Repeat | ImageExtend::Reflect
    );
    let mut content = String::new();
    unit(
        &mut content,
        id,
        source_id,
        m.geometry.extend_x,
        m.geometry.extend_y,
        range,
        edges,
    )?;
    if rx || ry {
        let x = if rx { 0.0 } else { range[0] - 1.0 };
        let y = if ry { 0.0 } else { range[1] - 1.0 };
        let w = if rx {
            if m.geometry.extend_x == ImageExtend::Reflect {
                2.0
            } else {
                1.0
            }
        } else {
            range[2] - range[0] + 2.0
        };
        let h = if ry {
            if m.geometry.extend_y == ImageExtend::Reflect {
                2.0
            } else {
                1.0
            }
        } else {
            range[3] - range[1] + 2.0
        };
        let _ = write!(
            out,
            r##"<defs><g id="image-unit-{id}">{content}</g><pattern id="image-repeat-{id}" patternUnits="userSpaceOnUse" x="{x}" y="{y}" width="{w}" height="{h}" viewBox="{x} {y} {w} {h}"><use href="#image-unit-{id}"/>"##
        );
        if m.geometry.extend_x == ImageExtend::Reflect {
            let _ = write!(
                out,
                r##"<use href="#image-unit-{id}" transform="translate(2 0) scale(-1 1)"/>"##
            );
        }
        if m.geometry.extend_y == ImageExtend::Reflect {
            let _ = write!(
                out,
                r##"<use href="#image-unit-{id}" transform="translate(0 2) scale(1 -1)"/>"##
            );
        }
        if m.geometry.extend_x == ImageExtend::Reflect
            && m.geometry.extend_y == ImageExtend::Reflect
        {
            let _ = write!(
                out,
                r##"<use href="#image-unit-{id}" transform="translate(2 2) scale(-1 -1)"/>"##
            );
        }
        out.push_str("</pattern></defs>");
        rectangle(
            out,
            [range[0], range[1], range[2] - range[0], range[3] - range[1]],
            &format!(r##"fill="url(#image-repeat-{id})""##),
        );
    } else {
        out.push_str(&content);
    }
    out.push_str("</g>");
    if clip {
        out.push_str("</g>");
    }
    Ok(())
}
fn unit(
    out: &mut String,
    id: &str,
    source: &str,
    x: ImageExtend,
    y: ImageExtend,
    range: [f64; 4],
    edges: Option<&EdgeSources>,
) -> Result<(), ImageSvgError> {
    let needed = x == ImageExtend::Pad && (range[0] < 0.0 || range[2] > 1.0)
        || y == ImageExtend::Pad && (range[1] < 0.0 || range[3] > 1.0);
    let _ = write!(out, r##"<use href="#{source}"/>"##);
    if !needed {
        return Ok(());
    }
    let e = edges.ok_or(ImageSvgError::SourcePending)?;
    let mut slice = |name: &str, src: &str, r: [f64; 4]| {
        if r[2] > 0.0 && r[3] > 0.0 {
            let [a, b, w, h] = r;
            let _ = write!(
                out,
                r##"<image id="image-edge-{name}-{id}" x="{a}" y="{b}" width="{w}" height="{h}" href="{}" preserveAspectRatio="none"/>"##,
                escape(src)
            );
        }
    };
    if x == ImageExtend::Pad {
        slice("left", &e.left, [range[0], 0.0, -range[0], 1.0]);
        slice("right", &e.right, [1.0, 0.0, range[2] - 1.0, 1.0]);
    }
    if y == ImageExtend::Pad {
        slice("top", &e.top, [0.0, range[1], 1.0, -range[1]]);
        slice("bottom", &e.bottom, [0.0, 1.0, 1.0, range[3] - 1.0]);
    }
    if x == ImageExtend::Pad && y == ImageExtend::Pad {
        for (name, src, r) in [
            (
                "tl",
                &e.corners[0],
                [range[0], range[1], -range[0], -range[1]],
            ),
            (
                "tr",
                &e.corners[1],
                [1.0, range[1], range[2] - 1.0, -range[1]],
            ),
            (
                "br",
                &e.corners[2],
                [1.0, 1.0, range[2] - 1.0, range[3] - 1.0],
            ),
            (
                "bl",
                &e.corners[3],
                [range[0], 1.0, -range[0], range[3] - 1.0],
            ),
        ] {
            slice(name, src, r);
        }
    }
    Ok(())
}
