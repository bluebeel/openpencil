//! Real image paint fields, retaining source parameters and encoded media.
use super::*;
use crate::layout_scene::SceneFillLayer;
use jian_ops_schema::{
    node::ImageSrc,
    style::{ImageFillBody, ImageFillMode, ImageOriginalSize, ImageTransform},
};
use op_editor_core::svg_image_composition::{self as composition, ImageSvgError};
use std::borrow::Cow;
fn original(node: &SceneNode) -> Cow<'_, ImageFillBody> {
    if let Some(body) = &node.image_paint {
        return Cow::Borrowed(body);
    }
    let a = node.image_adjustments;
    Cow::Owned(ImageFillBody {
        url: node
            .image_src
            .as_ref()
            .map(|s| ImageSrc::from_arc(s.clone()))
            .unwrap_or_default(),
        thumbnail_url: node
            .image_thumbnail_src
            .as_ref()
            .map(|s| ImageSrc::from_arc(s.clone())),
        geometry: node.image_geometry.as_deref().cloned().map(Box::new),
        nearest_sampling: Some(node.image_nearest_sampling),
        mode: Some(match node.image_fit {
            SceneImageFit::Fit => ImageFillMode::Fit,
            SceneImageFit::Fill => ImageFillMode::Fill,
            SceneImageFit::Crop => ImageFillMode::Crop,
            SceneImageFit::Tile => ImageFillMode::Tile,
            SceneImageFit::Stretch => ImageFillMode::Stretch,
        }),
        transform: node.image_transform.map(|m| ImageTransform {
            m00: m[0] as f64,
            m01: m[1] as f64,
            m02: m[2] as f64,
            m10: m[3] as f64,
            m11: m[4] as f64,
            m12: m[5] as f64,
        }),
        original_size: node.image_original_size.map(|s| ImageOriginalSize {
            width: s[0] as f64,
            height: s[1] as f64,
        }),
        rotation: node.image_rotation_degrees.map(f64::from),
        tile_scale: Some(node.image_tile_scale as f64),
        exposure: Some(a.exposure as f64),
        contrast: Some(a.contrast as f64),
        saturation: Some(a.saturation as f64),
        temperature: Some(a.temperature as f64),
        tint: Some(a.tint as f64),
        highlights: Some(a.highlights as f64),
        shadows: Some(a.shadows as f64),
        ..Default::default()
    })
}
fn dimensions(source: &str) -> Option<[u32; 2]> {
    let bytes = crate::widgets::canvas_viewport_image::image_source_bytes(
        source,
        crate::layout_scene::stable_image_source_id(source),
    )?;
    composition::source_dimensions(&bytes)
}
fn bbox(rect: Rect) -> [f64; 4] {
    [
        rect.origin.x as f64,
        rect.origin.y as f64,
        rect.size.x as f64,
        rect.size.y as f64,
    ]
}
/// A source thumbnail is selected only where the native two-grid paint is
/// physically adequate; the original remains present for lossless restoration.
fn preview(body: &ImageFillBody, bounds: [f64; 4]) -> bool {
    if body.geometry.is_some() || body.nearest_sampling != Some(true) {
        return false;
    }
    let Some(src) = body.thumbnail_url.as_deref() else {
        return false;
    };
    let Some(d) = dimensions(src) else {
        return false;
    };
    let Ok(g) = composition::legacy_geometry(body, bounds, Some(d)) else {
        return false;
    };
    let Ok(p) = op_editor_core::image_field_geometry::prepare_with_source(&g, bounds, d) else {
        return false;
    };
    let [a, b, c, e, _, _] = p.image_to_document.as_coeffs();
    let x = [a / d[0] as f64, b / d[0] as f64];
    let y = [c / d[1] as f64, e / d[1] as f64];
    let trace = x[0] * x[0] + x[1] * x[1] + y[0] * y[0] + y[1] * y[1];
    let determinant = x[0] * y[1] - x[1] * y[0];
    let maximum = (trace
        + (trace * trace - 4.0 * determinant * determinant)
            .max(0.0)
            .sqrt())
        * 0.5;
    maximum * 4.0 <= 1.0 + 1e-6
}
pub(super) fn definition(
    id: &str,
    node: &SceneNode,
    bounds: Rect,
    coverage: Rect,
) -> Result<String, ImageSvgError> {
    let body = original(node);
    let b = bbox(bounds);
    let thumbnail = preview(&body, b);
    let source = if thumbnail {
        body.thumbnail_url.as_deref().unwrap()
    } else {
        body.url.as_ref()
    };
    let d = dimensions(source);
    let geometry = composition::legacy_geometry(&body, b, d)?;
    let metadata = composition::metadata(
        &body,
        geometry.clone(),
        b,
        bbox(coverage),
        d,
        node.opacity as f64,
        thumbnail,
    );
    match composition::definition(
        id,
        &metadata,
        &body.url,
        body.thumbnail_url.as_deref(),
        None,
    ) {
        Ok(svg) => Ok(svg),
        Err(ImageSvgError::SourcePending) => {
            let bytes = crate::widgets::canvas_viewport_image::image_source_bytes(
                source,
                crate::layout_scene::stable_image_source_id(source),
            )
            .ok_or(ImageSvgError::SourcePending)?;
            let edges = composition::source_edges(&bytes)?;
            composition::definition(
                id,
                &metadata,
                &body.url,
                body.thumbnail_url.as_deref(),
                Some(&edges),
            )
        }
        Err(error) => Err(error),
    }
}
pub(super) fn emit_node_definition(
    out: &mut String,
    node: &SceneNode,
) -> Result<(), SvgExportError> {
    let mut image = SceneNode::leaf("", NodeKind::Rect);
    image.image_src = node.image_src.clone();
    image.image_src_id = node.image_src_id;
    image.image_paint = node.image_paint.clone();
    image.image_geometry = node.image_geometry.clone();
    image.image_fit = node.image_fit;
    image.image_transform = node.image_transform;
    image.image_rotation_degrees = node.image_rotation_degrees;
    image.image_original_size = node.image_original_size;
    image.image_tile_scale = node.image_tile_scale;
    image.image_nearest_sampling = node.image_nearest_sampling;
    image.image_thumbnail_src = node.image_thumbnail_src.clone();
    image.image_thumbnail_src_id = node.image_thumbnail_src_id;
    image.image_adjustments = node.image_adjustments;
    image.opacity = node.opacity;
    if let Some(body) = &image.image_paint {
        image.opacity *= body.opacity.unwrap_or(1.0) as f32;
    }
    out.push_str(
        &definition(
            &format!("image-field-{}", svg_id(&node.id)),
            &image,
            node.bounds,
            node.bounds,
        )
        .map_err(|source| SvgExportError::InvalidImage {
            node_id: node.id.clone(),
            source,
        })?,
    );
    Ok(())
}
pub(super) fn layer_node(parent: &SceneNode, layer: &SceneFillLayer) -> SceneNode {
    let mut n = SceneNode::leaf("", NodeKind::Rect);
    n.bounds = parent.bounds;
    if let SceneFillLayer::Image {
        src,
        src_id,
        thumbnail_src,
        thumbnail_src_id,
        nearest_sampling,
        fit,
        transform,
        original_size,
        rotation_degrees,
        tile_scale,
        adjustments,
        opacity,
        geometry,
        paint,
        ..
    } = layer
    {
        n.image_src = Some(src.clone());
        n.image_src_id = *src_id;
        n.image_thumbnail_src = thumbnail_src.clone();
        n.image_thumbnail_src_id = *thumbnail_src_id;
        n.image_nearest_sampling = *nearest_sampling;
        n.image_fit = *fit;
        n.image_transform = *transform;
        n.image_original_size = *original_size;
        n.image_rotation_degrees = *rotation_degrees;
        n.image_tile_scale = *tile_scale;
        n.image_adjustments = *adjustments;
        n.opacity = *opacity;
        n.image_geometry = geometry.clone();
        n.image_paint = paint.clone();
    }
    n
}
