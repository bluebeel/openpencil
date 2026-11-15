//! Explicit source/tile affine fields; legacy Figma image sampling is separate.
use super::{
    image::image_adjustment_matrix, layer::to_skia_blend_mode, raster_point, to_sk_rect,
    NativeBackend,
};
use op_editor_core::image_field_geometry::prepare_with_source;
use op_editor_ui::{ImageExtend, ImagePaintOptions, Rect};
use skia_safe::{runtime_effect::ChildPtr, Canvas, Data, Matrix, RuntimeEffect};
const SOURCE: &str = op_editor_core::image_field_geometry::PATTERN_SHADER;
thread_local! {
static EFFECT:Option<RuntimeEffect>=RuntimeEffect::make_for_shader(SOURCE,None).ok();
static SOURCE_EFFECT:Option<RuntimeEffect>=RuntimeEffect::make_for_shader(op_editor_core::image_field_geometry::SOURCE_SHADER,None).ok();
}
fn tile(mode: ImageExtend) -> skia_safe::TileMode {
    match mode {
        ImageExtend::Decal => skia_safe::TileMode::Clamp,
        ImageExtend::Pad => skia_safe::TileMode::Clamp,
        ImageExtend::Repeat => skia_safe::TileMode::Repeat,
        ImageExtend::Reflect => skia_safe::TileMode::Mirror,
    }
}
fn code(mode: ImageExtend) -> f32 {
    match mode {
        ImageExtend::Decal => 0.0,
        ImageExtend::Pad => 1.0,
        ImageExtend::Repeat => 2.0,
        ImageExtend::Reflect => 3.0,
    }
}
pub(super) fn draw(
    backend: &mut NativeBackend,
    canvas: &Canvas,
    coverage: Rect,
    id: u64,
    encoded: &[u8],
    options: &ImagePaintOptions<'_>,
) {
    let Some(geometry) = options.geometry else {
        return;
    };
    if geometry.units == op_editor_ui::ImageFieldUnits::LocalPixels
        && geometry
            .viewport
            .as_ref()
            .is_some_and(|v| v.size.contains(&0.0))
    {
        jian_skia::raster_sampling::mark_unsupported(
            "SVG image intrinsic layout is pending media resolution",
        );
        return;
    }

    let rect = backend.active_paint_rect(coverage);
    let Some(image) = backend.raster_image(id) else {
        return;
    };
    let requires_aspect = geometry.viewport.as_ref().is_some_and(|viewport| {
        viewport.fit != op_editor_ui::ImageViewportFit::None || viewport.size.contains(&0.0)
    });
    // In normalized units with a concrete, non-fitting viewport the shared
    // geometry preparation never consumes source dimensions. Avoid reparsing
    // SVG or copying the entire encoded raster for unused intrinsic metadata.
    let requires_intrinsic =
        geometry.units != op_editor_ui::ImageFieldUnits::Normalized || requires_aspect;
    let intrinsic = if requires_intrinsic && !encoded.is_empty() {
        op_editor_core::svg_image_composition::source_dimensions(encoded).or_else(|| {
            skia_safe::Image::from_encoded(skia_safe::Data::new_copy(encoded))
                .map(|value| [value.width() as u32, value.height() as u32])
        })
    } else {
        None
    };
    if requires_aspect && intrinsic.is_none() {
        jian_skia::raster_sampling::mark_unsupported("SVG image intrinsic source is pending");
        return;
    }
    let source_dimensions = intrinsic.unwrap_or([image.width() as u32, image.height() as u32]);
    let setup = match prepare_with_source(
        geometry,
        [
            rect.origin.x as f64,
            rect.origin.y as f64,
            rect.size.x as f64,
            rect.size.y as f64,
        ],
        source_dimensions,
    ) {
        Ok(v) => v,
        Err(_) => {
            jian_skia::raster_sampling::mark_unsupported("invalid explicit image field");
            return;
        }
    };
    if super::image_supersample::draw(
        canvas,
        to_sk_rect(coverage),
        options.opacity,
        to_skia_blend_mode(options.blend_mode),
        |inner| {
            let inner_options = ImagePaintOptions {
                opacity: 1.0,
                blend_mode: op_editor_ui::ImageBlendMode::Normal,
                ..*options
            };
            draw(backend, inner, coverage, id, encoded, &inner_options);
        },
    ) {
        return;
    }
    let coefficients = setup.image_to_document.as_coeffs();
    let local = [
        coefficients[0] / image.width() as f64,
        coefficients[2] / image.height() as f64,
        coefficients[4],
        coefficients[1] / image.width() as f64,
        coefficients[3] / image.height() as f64,
        coefficients[5],
    ]
    .map(|v| v as f32);
    if !local.iter().all(|v| v.is_finite()) {
        jian_skia::raster_sampling::mark_unsupported(
            "explicit image field exceeds backend precision",
        );
        return;
    }
    let source = if options.nearest_sampling {
        image
    } else {
        super::image_precision::minification_source(&image, canvas, local)
    };
    let local = Matrix::new_all(
        local[0], local[1], local[2], local[3], local[4], local[5], 0.0, 0.0, 1.0,
    );
    let filter = if options.nearest_sampling {
        skia_safe::FilterMode::Nearest
    } else {
        skia_safe::FilterMode::Linear
    };
    let Some(child) = source.to_shader(
        (tile(setup.source_extend[0]), tile(setup.source_extend[1])),
        skia_safe::SamplingOptions::new(filter, skia_safe::MipmapMode::Linear),
        &local,
    ) else {
        return;
    };
    let child = if setup.patterned {
        let [a, b, c, d, e, f] = setup.image_to_document.inverse().as_coeffs();
        let [va, vb, vc, vd, ve, vf] = setup.viewport_to_document.inverse().as_coeffs();
        let values = [
            a,
            c,
            b,
            d,
            e,
            f,
            va,
            vc,
            vb,
            vd,
            ve,
            vf,
            if setup.source_extend[0] == ImageExtend::Decal {
                1.0
            } else {
                0.0
            },
            if setup.source_extend[1] == ImageExtend::Decal {
                1.0
            } else {
                0.0
            },
            if setup.clip_viewport { 1.0 } else { 0.0 },
        ]
        .map(|v| v as f32);
        let bytes: [u8; 60] =
            std::array::from_fn(|index| values[index / 4].to_ne_bytes()[index % 4]);
        let shader = SOURCE_EFFECT.with(|effect| {
            effect
                .as_ref()?
                .make_shader(Data::new_copy(&bytes), &[ChildPtr::Shader(child)], None)
        });
        let Some(shader) = shader else {
            jian_skia::raster_sampling::mark_unsupported("image source support shader unavailable");
            return;
        };
        shader
    } else {
        child
    };
    let shader = if setup.patterned {
        let [a, b, c, d, e, f] = setup.document_to_tile.as_coeffs();
        let [ta, tb, tc, td, te, tf] = setup.tile_to_document.as_coeffs();
        let [lo_x, lo_y, hi_x, hi_y] = setup.tile_source_bounds;
        let values = [
            a,
            c,
            b,
            d,
            e,
            f,
            ta,
            tc,
            tb,
            td,
            te,
            tf,
            lo_x,
            lo_y,
            hi_x,
            hi_y,
            code(setup.pattern_extend[0]) as f64,
            code(setup.pattern_extend[1]) as f64,
            if setup.clip_tile { 1.0 } else { 0.0 },
        ];
        let values = values.map(|v| v as f32);
        if !values.iter().all(|v| v.is_finite()) {
            return;
        }
        let bytes: [u8; 76] =
            std::array::from_fn(|index| values[index / 4].to_ne_bytes()[index % 4]);
        let shader = EFFECT.with(|effect| {
            effect.as_ref()?.make_shader(
                Data::new_copy(&bytes),
                &[ChildPtr::Shader(child)],
                Some(&Matrix::default()),
            )
        });
        let Some(shader) = shader else {
            jian_skia::raster_sampling::mark_unsupported(
                "explicit image pattern effect unavailable",
            );
            return;
        };
        shader
    } else {
        child
    };
    let mut paint = skia_safe::Paint::default();
    jian_skia::raster_sampling::configure_geometry(&mut paint);
    paint.set_shader(shader);
    paint.set_alpha_f(options.opacity.clamp(0.0, 1.0));
    jian_skia::raster_sampling::set_blend_mode(&mut paint, to_skia_blend_mode(options.blend_mode));
    if let Some(m) = image_adjustment_matrix(options.adjustments) {
        paint.set_color_filter(skia_safe::color_filters::matrix_row_major(&m, None));
    }
    let save = canvas.save();
    let mut draw_bounds = coverage;
    if !setup.patterned {
        if geometry.viewport.as_ref().is_some_and(|v| !v.clip)
            && setup.source_extend == [ImageExtend::Decal; 2]
        {
            if let Some([x, y, w, h]) = op_editor_core::image_field_geometry::source_bounds(&setup)
            {
                draw_bounds = Rect::xywh(x as f32, y as f32, w as f32, h as f32);
            }
        }
        let polygon = op_editor_core::image_field_geometry::visible_polygon(
            &setup,
            [
                draw_bounds.origin.x as f64,
                draw_bounds.origin.y as f64,
                draw_bounds.size.x as f64,
                draw_bounds.size.y as f64,
            ],
        )
        .unwrap_or_default();
        if polygon.len() < 3 {
            canvas.restore_to_count(save);
            return;
        }
        let mut path = skia_safe::PathBuilder::new();
        path.move_to((polygon[0][0] as f32, polygon[0][1] as f32));
        for p in &polygon[1..] {
            path.line_to((p[0] as f32, p[1] as f32));
        }
        path.close();
        raster_point::clip_path(canvas, &path.detach());
        if options.corner_radius > 0.0 {
            let r = skia_safe::RRect::new_rect_xy(
                to_sk_rect(coverage),
                options.corner_radius,
                options.corner_radius,
            );
            super::sampled_clip::clip_rrect(canvas, &r);
        }
        let pad = 1.0;
        draw_bounds = Rect::xywh(
            draw_bounds.origin.x - pad,
            draw_bounds.origin.y - pad,
            draw_bounds.size.x + 2.0 * pad,
            draw_bounds.size.y + 2.0 * pad,
        );
        canvas.draw_rect(to_sk_rect(draw_bounds), &paint);
    } else {
        raster_point::draw_round_rect(
            canvas,
            to_sk_rect(draw_bounds),
            options.corner_radius,
            options.corner_radius,
            &paint,
        );
    }
    canvas.restore_to_count(save);
}
