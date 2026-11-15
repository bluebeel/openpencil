//! Restore original typed paint parameters only after complete visual proof.
use super::*;
use crate::svg_image_composition as composition;
use jian_ops_schema::node::ImageSrc;
use kurbo::Affine;
pub(super) fn restore_image(
    resources: &SvgResources<'_>,
    entry: &Definition<'_>,
    opacity: f64,
    space: PaintSpace,
) -> Result<Option<PenFill>, SvgResourceError> {
    let invalid = |attribute| SvgResourceError::Invalid { attribute };
    if resources.has_stylesheets || !resources.duplicate_ids.is_empty() {
        return Err(invalid("image restoration with ambiguous SVG definitions"));
    }
    let raw = resource_attr(entry.node, composition::METADATA_ATTRIBUTE)
        .ok_or_else(|| invalid("image metadata"))?;
    let metadata = composition::decode_metadata(raw)
        .map_err(|source| SvgResourceError::ImageMetadata { source })?;
    let id = entry
        .node
        .attr("id")
        .ok_or_else(|| invalid("image resource ID"))?;
    let source = |name: &str| -> Option<&str> {
        let key = format!("image-source-{name}-{id}");
        let entry = resources.entries.get(key.as_str())?;
        resource_attr(entry.node, "href").or_else(|| resource_attr(entry.node, "xlink:href"))
    };
    let original = source("original").ok_or_else(|| invalid("original image reference"))?;
    let thumbnail = source("thumbnail");
    let display = if metadata.display_thumbnail {
        thumbnail.ok_or_else(|| invalid("image display preview"))?
    } else {
        original
    };
    if let (Some(expected), Some(bytes)) = (metadata.dimensions, uri_bytes(display)) {
        if composition::source_dimensions(&bytes).is_some_and(|d| expected != d) {
            return Err(invalid("image source dimensions changed"));
        }
    }
    let expected = match composition::definition(id, &metadata, original, thumbnail, None) {
        Ok(svg) => svg,
        Err(composition::ImageSvgError::SourcePending) => {
            let bytes =
                uri_bytes(display).ok_or_else(|| invalid("image edge source is pending"))?;
            let edges = composition::source_edges(&bytes)
                .map_err(|source| SvgResourceError::ImageMetadata { source })?;
            composition::definition(id, &metadata, original, thumbnail, Some(&edges))
                .map_err(|source| SvgResourceError::ImageMetadata { source })?
        }
        Err(source) => return Err(SvgResourceError::ImageMetadata { source }),
    };
    let expected = parse_svg_tree(&expected);
    if expected.len() != 1 || !same_visual(&expected[0], entry.node) {
        return Err(invalid("image visual definition changed"));
    }
    let mut body = metadata.paint;
    body.url = ImageSrc::from(original);
    body.thumbnail_url = thumbnail.map(ImageSrc::from);
    let old = metadata.bounds;
    let target = [
        space.target.x,
        space.target.y,
        space.target.w,
        space.target.h,
    ];
    let [a, b, c, d, _, _] = space.matrix.as_coeffs();
    let source_bounds = [
        space.source.x,
        space.source.y,
        space.source.w,
        space.source.h,
    ];
    // A positive uniform SVG placement preserves normalized legacy parameters.
    // Other transforms receive an explicit rebased field; original mode/crop/
    // rotation parameters remain present rather than being rounded away.
    if !(a > 0.0 && a == d && b == 0.0 && c == 0.0 && source_bounds == old) {
        body.geometry = Some(Box::new(
            crate::image_field_geometry::rebase(&metadata.geometry, old, space.matrix, target)
                .map_err(|source| SvgResourceError::ImageMetadata {
                    source: composition::ImageSvgError::InvalidGeometry { source },
                })?,
        ));
    }
    let original_opacity = body.opacity.unwrap_or(1.0);
    if opacity != 1.0 || f64::from(original_opacity as f32) != metadata.render_opacity {
        body.opacity = Some((metadata.render_opacity * opacity).clamp(0.0, 1.0));
    }
    Ok(Some(PenFill::Image(Box::new(body))))
}
