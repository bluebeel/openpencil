use super::CanvasKitBackend;
use op_editor_ui::{ImageExtend, ImagePaintOptions, ImageViewportFit, Rect, RenderBackend};
fn code(e: ImageExtend) -> f64 {
    match e {
        ImageExtend::Decal => 0.0,
        ImageExtend::Pad => 1.0,
        ImageExtend::Repeat => 2.0,
        ImageExtend::Reflect => 3.0,
    }
}
pub(super) fn draw(
    backend: &mut CanvasKitBackend,
    rect: Rect,
    id: u64,
    encoded: &[u8],
    options: &ImagePaintOptions<'_>,
) {
    let Some(g) = options.geometry else {
        backend.draw_image_with_options_transform_blend_and_tile_scale(
            rect,
            id,
            encoded,
            options.mode,
            options.adjustments,
            options.opacity,
            options.corner_radius,
            options.transform,
            options.blend_mode,
            options.original_size,
            options.tile_scale,
        );
        return;
    };
    let mut values = [0.0; 28];
    values[..6].copy_from_slice(&g.transform);
    values[6..9].copy_from_slice(&[
        code(g.extend_x),
        code(g.extend_y),
        if g.pattern.is_some() { 1.0 } else { 0.0 },
    ]);
    if let Some(pattern) = &g.pattern {
        values[9..15].copy_from_slice(&pattern.transform);
        values[15..18].copy_from_slice(&[
            code(pattern.extend_x.extension()),
            code(pattern.extend_y.extension()),
            if pattern.clip { 1.0 } else { 0.0 },
        ]);
    } else {
        values[9..18].copy_from_slice(&[1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
    }
    if let Some(viewport) = &g.viewport {
        values[18..25].copy_from_slice(&[
            1.0,
            viewport.size[0],
            viewport.size[1],
            match viewport.fit {
                ImageViewportFit::None => 0.0,
                ImageViewportFit::Meet => 1.0,
                ImageViewportFit::Slice => 2.0,
            },
            viewport.align[0],
            viewport.align[1],
            if viewport.clip { 1.0 } else { 0.0 },
        ]);
    } else {
        values[18..25].copy_from_slice(&[0.0, 1.0, 1.0, 0.0, 0.5, 0.5, 0.0]);
    }
    values[25] = match g.units {
        op_editor_ui::ImageFieldUnits::Normalized => 0.0,
        op_editor_ui::ImageFieldUnits::UserSpace => 1.0,
        op_editor_ui::ImageFieldUnits::LocalPixels => 2.0,
    };
    let requires_intrinsic = g.units != op_editor_ui::ImageFieldUnits::Normalized
        || g.viewport.as_ref().is_some_and(|viewport| {
            viewport.fit != ImageViewportFit::None || viewport.size.contains(&0.0)
        });
    let dimensions = if requires_intrinsic {
        op_editor_core::svg_image_composition::source_dimensions(encoded).unwrap_or([0, 0])
    } else {
        [0, 0]
    };
    values[26..28].copy_from_slice(&dimensions.map(f64::from));
    let a = options.adjustments;
    let adjustments = [
        a.exposure,
        a.contrast,
        a.saturation,
        a.temperature,
        a.tint,
        a.highlights,
        a.shadows,
    ];
    backend.ck.draw_explicit_image_field(
        id as u32,
        (id >> 32) as u32,
        rect.origin.x,
        rect.origin.y,
        rect.size.x,
        rect.size.y,
        &values,
        &adjustments,
        options.opacity,
        options.corner_radius,
        super::super::convert::image_blend_mode_code(options.blend_mode),
        options.nearest_sampling,
        op_editor_core::image_field_geometry::SOURCE_SHADER,
        op_editor_core::image_field_geometry::PATTERN_SHADER,
    );
}
