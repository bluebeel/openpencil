//! Restore typed gradients only when their complete generated visual subtree
//! is unchanged. Metadata is a parameter packet, never a visual override.

use super::*;
use crate::svg_gradient_composition::{self as composition, METADATA_ATTRIBUTE};
use jian_ops_schema::style::{GradientShape, LinearGradientBody, RadialGradientBody};
use kurbo::Affine;
use std::collections::BTreeMap;

pub(super) fn restore_gradient(
    resources: &SvgResources<'_>,
    entry: &Definition<'_>,
    opacity: f64,
    space: PaintSpace,
) -> Result<Option<PenFill>, SvgResourceError> {
    if resources.has_stylesheets || !resources.duplicate_ids.is_empty() {
        return Err(SvgResourceError::Unsupported {
            feature: "typed gradient restoration with stylesheets or duplicate IDs".into(),
        });
    }
    let raw = resource_attr(entry.node, METADATA_ATTRIBUTE).ok_or(SvgResourceError::Invalid {
        attribute: "typed gradient metadata",
    })?;
    let metadata = composition::decode_metadata(raw)
        .map_err(|source| SvgResourceError::GradientMetadata { source })?;
    let id = entry.node.attr("id").ok_or(SvgResourceError::Invalid {
        attribute: "typed gradient ID",
    })?;
    let expected = composition::definition_with_opacity(
        id,
        &metadata.paint,
        metadata.bounds,
        metadata.render_opacity,
    )
    .map_err(|source| SvgResourceError::GradientMetadata { source })?;
    let expected = parse_svg_tree(&expected);
    if expected.len() != 1 || !same_visual(&expected[0], entry.node) {
        return Err(SvgResourceError::Invalid {
            attribute: "typed gradient visual definition changed",
        });
    }
    let mut geometry = metadata.paint.geometry;
    let [x, y, w, h] = metadata.bounds;
    let target = [
        space.target.x,
        space.target.y,
        space.target.w,
        space.target.h,
    ];
    if space.matrix != Affine::IDENTITY || target != metadata.bounds {
        let basis = Affine::translate((x, y)) * Affine::scale_non_uniform(w, h);
        let normalized = Affine::scale_non_uniform(1.0 / space.target.w, 1.0 / space.target.h)
            * Affine::translate((-space.target.x, -space.target.y))
            * space.matrix
            * basis
            * Affine::new(geometry.transform);
        if !normalized.as_coeffs().iter().all(|v| v.is_finite()) {
            return Err(SvgResourceError::Invalid {
                attribute: "typed gradient placement",
            });
        }
        geometry.transform = normalized.as_coeffs();
    }
    let original = metadata.paint.opacity;
    let source_opacity = if f64::from(original as f32) == metadata.render_opacity {
        original
    } else {
        metadata.render_opacity
    };
    let opacity = Some((source_opacity * opacity).clamp(0.0, 1.0));
    let stops = metadata.paint.stops;
    Ok(Some(match geometry.shape {
        GradientShape::Linear { .. } => PenFill::LinearGradient(LinearGradientBody {
            enabled: None,
            geometry: Some(Box::new(geometry)),
            angle: None,
            stops,
            opacity,
            explain: None,
            blend_mode: None,
        }),
        GradientShape::Radial { .. } => PenFill::RadialGradient(RadialGradientBody {
            enabled: None,
            geometry: Some(Box::new(geometry)),
            cx: None,
            cy: None,
            radius: None,
            stops,
            opacity,
            explain: None,
            blend_mode: None,
        }),
    }))
}

pub(super) fn same_visual(expected: &SvgTree, actual: &SvgTree) -> bool {
    if expected.source_tag != actual.source_tag || expected.children.len() != actual.children.len()
    {
        return false;
    }
    if visual_attributes(expected) != visual_attributes(actual) {
        return false;
    }
    expected
        .children
        .iter()
        .zip(&actual.children)
        .all(|(expected, actual)| same_visual(expected, actual))
}

fn visual_attributes(node: &SvgTree) -> Option<BTreeMap<&str, &str>> {
    let mut out = BTreeMap::new();
    for (key, value) in &node.attrs {
        if key.eq_ignore_ascii_case(METADATA_ATTRIBUTE)
            || key.eq_ignore_ascii_case(crate::svg_image_composition::METADATA_ATTRIBUTE)
        {
            continue;
        }
        if out.insert(key.as_str(), value.as_str()).is_some() {
            return None;
        }
    }
    Some(out)
}
