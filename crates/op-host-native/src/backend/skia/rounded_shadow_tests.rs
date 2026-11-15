use super::*;
use crate::backend::NativeFrameBackend;
use op_editor_ui::{RenderBackend, RoundRectShadowOptions, ShadowBlurKernel};

fn options(kernel: ShadowBlurKernel) -> RoundRectShadowOptions {
    RoundRectShadowOptions {
        offset: Point2D::ZERO,
        blur: 8.0,
        spread: 0.0,
        color: Color::BLACK,
        kernel,
        inner: false,
    }
}
fn surface() -> skia_safe::Surface {
    let mut s = skia_safe::surfaces::raster_n32_premul((128, 128)).unwrap();
    s.canvas().clear(skia_safe::Color::WHITE);
    s
}
fn bytes(surface: &mut skia_safe::Surface) -> Vec<u8> {
    let info = skia_safe::ImageInfo::new(
        (128, 128),
        skia_safe::ColorType::RGBA8888,
        skia_safe::AlphaType::Unpremul,
        None,
    );
    let mut bytes = vec![0; 128 * 128 * 4];
    assert!(surface.image_snapshot().read_pixels(
        &info,
        &mut bytes,
        128 * 4,
        (0, 0),
        skia_safe::image::CachingHint::Allow
    ));
    bytes
}
fn at(bytes: &[u8], x: usize, y: usize) -> u8 {
    bytes[(y * 128 + x) * 4]
}

#[test]
fn gaussian_options_and_zero_blur_keep_the_legacy_raster() {
    let mut backend = NativeBackend::with_dpi(1.0);
    let rect = Rect::xywh(24.0, 24.0, 40.0, 40.0);
    for radius in [0.0, 7.0, 20.0] {
        for blur in [0.0, 8.0] {
            let mut expected = surface();
            backend.fill_drop_shadow(expected.canvas(), rect, radius, blur, Color::BLACK);
            let mut actual = surface();
            let mut opts = options(ShadowBlurKernel::Gaussian);
            opts.blur = blur;
            assert!(backend.fill_round_rect_shadow(actual.canvas(), rect, radius, &opts));
            assert_eq!(bytes(&mut actual), bytes(&mut expected));
            if blur == 0.0 {
                let mut sharp = surface();
                opts.kernel = ShadowBlurKernel::Triangular;
                assert!(backend.fill_round_rect_shadow(sharp.canvas(), rect, radius, &opts));
                assert_eq!(bytes(&mut sharp), bytes(&mut expected));
            }
        }
    }
}

#[test]
fn triangular_square_shadow_has_analytic_falloff_and_finite_support_at_each_scale() {
    let mut backend = NativeBackend::with_dpi(1.0);
    for scale in [1.0, 2.0, 3.0] {
        let mut image = surface();
        image.canvas().scale((scale, scale));
        let rect = Rect::xywh(8.0, 8.0, 20.0, 20.0);
        assert!(backend.fill_round_rect_shadow(
            image.canvas(),
            rect,
            0.0,
            &options(ShadowBlurKernel::Triangular)
        ));
        let actual = bytes(&mut image);
        let y = (18.0 * scale) as usize;
        for x in (28.0 * scale) as usize..(39.0 * scale) as usize {
            let distance = ((x as f64 + 0.5) / f64::from(scale) - 28.0) / 8.0;
            let coverage = 0.5 * (1.0 - distance.clamp(0.0, 1.0)).powi(2);
            let expected = (255.0 * (1.0 - coverage)).round() as u8;
            assert!(
                at(&actual, x, y).abs_diff(expected) <= 1,
                "scale{scale},x{x}"
            );
        }
        assert_eq!(at(&actual, (38.0 * scale) as usize, y), 255);
    }
}

#[test]
fn rounded_triangular_shadow_keeps_float_color_until_framebuffer_composition() {
    let mut backend = NativeBackend::with_dpi(1.0);
    let mut image = surface();
    image.canvas().clear(skia_safe::Color::from_rgb(11, 37, 83));
    let mut opts = options(ShadowBlurKernel::Triangular);
    opts.color = Color {
        r: 190.0 / 255.0,
        g: 41.0 / 255.0,
        b: 77.0 / 255.0,
        a: 0.35,
    };
    assert!(backend.fill_round_rect_shadow(
        image.canvas(),
        Rect::xywh(8.0, 8.0, 80.0, 80.0),
        12.0,
        &opts
    ));
    let bytes = bytes(&mut image);
    assert_eq!(
        &bytes[(40 * 128 + 40) * 4..(40 * 128 + 40) * 4 + 4],
        &[74, 38, 81, 255]
    );
}

#[test]
fn rounded_shadow_profiles_honor_signed_spread_and_inset_collapse() {
    let mut backend = NativeBackend::with_dpi(1.0);
    let rect = Rect::xywh(40.0, 40.0, 20.0, 20.0);
    for kernel in [ShadowBlurKernel::Gaussian, ShadowBlurKernel::Triangular] {
        let mut outer = options(kernel);
        outer.blur = 0.0;
        outer.spread = -10.0;
        let mut image = surface();
        assert!(backend.fill_round_rect_shadow(image.canvas(), rect, 4.0, &outer));
        assert!(bytes(&mut image).iter().all(|v| *v == 255));
        let mut inset = options(kernel);
        inset.inner = true;
        inset.offset = Point2D::new(6.0, 0.0);
        inset.blur = 0.0;
        let mut image = surface();
        assert!(backend.fill_round_rect_shadow(image.canvas(), rect, 0.0, &inset));
        let pixels = bytes(&mut image);
        assert_eq!(at(&pixels, 42, 50), 0);
        assert_eq!(at(&pixels, 52, 50), 255);
        assert_eq!(at(&pixels, 38, 50), 255);
        inset.spread = 10.0;
        let mut image = surface();
        assert!(backend.fill_round_rect_shadow(image.canvas(), rect, 0.0, &inset));
        assert_eq!(at(&bytes(&mut image), 52, 50), 0);
        inset.spread = -10.0;
        let mut image = surface();
        assert!(backend.fill_round_rect_shadow(image.canvas(), rect, 0.0, &inset));
        assert_eq!(at(&bytes(&mut image), 42, 50), 255);
    }
}

#[test]
fn triangular_insets_stay_inside_the_original_rounded_mask() {
    let mut backend = NativeBackend::with_dpi(1.0);
    for radius in [0.0, 4.0, 12.0] {
        for blur in [2.0, 8.0, 24.0] {
            let mut opts = options(ShadowBlurKernel::Triangular);
            opts.inner = true;
            opts.blur = blur;
            opts.offset = Point2D::new(4.0, 3.0);
            let mut image = surface();
            let saved = image.canvas().save_count();
            assert!(backend.fill_round_rect_shadow(
                image.canvas(),
                Rect::xywh(40.0, 40.0, 24.0, 24.0),
                radius,
                &opts
            ));
            assert_eq!(image.canvas().save_count(), saved);
            let pixels = bytes(&mut image);
            assert_eq!(at(&pixels, 30, 50), 255);
            assert_eq!(at(&pixels, 68, 50), 255);
            assert!(at(&pixels, 42, 52) < 255);
        }
    }
}

#[test]
fn triangular_frame_offsets_are_invariant_under_rotation_reflection_and_scale() {
    for (degrees, model_scale) in [
        (30.0f32, Point2D::new(1.0, 1.0)),
        (-30.0, Point2D::new(-1.0, 1.0)),
        (90.0, Point2D::new(1.5, 0.75)),
    ] {
        let mut backend = NativeBackend::with_dpi(1.0);
        let mut actual = surface();
        let mut expected = surface();
        let rect = Rect::xywh(30.0, 30.0, 24.0, 24.0);
        let pivot = Point2D::new(42.0, 42.0);
        let offset = Point2D::new(5.0, 7.0);
        let mut opts = options(ShadowBlurKernel::Triangular);
        {
            let canvas = actual.canvas();
            let mut frame = NativeFrameBackend::new(&mut backend, canvas);
            frame.scale(model_scale, pivot);
            frame.rotate(degrees.to_radians(), pivot);
            opts.offset = frame.shadow_offset_in_local_space(offset);
            assert!(frame.fill_round_rect_shadow(rect, 5.0, &opts));
        }
        {
            let canvas = expected.canvas();
            canvas.translate((offset.x, offset.y));
            canvas.translate((pivot.x, pivot.y));
            canvas.scale((model_scale.x, model_scale.y));
            canvas.rotate(degrees.to_radians().to_degrees(), None);
            canvas.translate((-pivot.x, -pivot.y));
            opts.offset = Point2D::ZERO;
            assert!(backend.fill_round_rect_shadow(canvas, rect, 5.0, &opts));
        }
        let a = bytes(&mut actual);
        let b = bytes(&mut expected);
        assert!(
            a.iter().zip(&b).all(|(a, b)| a.abs_diff(*b) <= 1),
            "{degrees}"
        );
    }
}

#[test]
fn interior_partition_matches_the_original_analytic_draw_exactly() {
    use jian_skia::triangular_shadow::{self, RoundedShadowDraw};
    for (angle, scale) in [
        (0.0, (1.0, 1.0)),
        (30.0, (1.5, 0.75)),
        (-30.0, (-1.0, 1.0)),
        (15.0, (0.1, 0.1)),
        (0.0, (3.0, 3.0)),
    ] {
        for alpha in [0.0, 0.05, 0.1, 0.35, 1.0] {
            for inner in [false, true] {
                for transparent in [false, true] {
                    let shape = skia_safe::Rect::from_xywh(20.0, 20.0, 64.0, 80.0);
                    let color =
                        skia_safe::Color4f::new(190.0 / 255.0, 41.0 / 255.0, 77.0 / 255.0, alpha);
                    let mut expected = surface();
                    let mut actual = surface();
                    for (image, optimized) in [(&mut expected, false), (&mut actual, true)] {
                        let canvas = image.canvas();
                        if transparent {
                            canvas.clear(skia_safe::Color::TRANSPARENT);
                        } else {
                            canvas.clear(skia_safe::Color::from_rgb(11, 37, 83));
                        }
                        canvas.translate((64.125, 64.375));
                        canvas.rotate(angle, None);
                        canvas.scale(scale);
                        canvas.translate((-52.0, -60.0));
                        // Include inherited fractional coverage; the partition
                        // must neither square it nor drop its partial alpha.
                        sampled_clip::clip_rrect(
                            canvas,
                            &skia_safe::RRect::new_rect_xy(
                                skia_safe::Rect::from_xywh(30.25, 24.75, 60.0, 80.0),
                                9.0,
                                9.0,
                            ),
                        );
                        let draw_bounds = shape.with_outset((8.0, 8.0));
                        if optimized {
                            let saved = canvas.save_count();
                            assert!(triangular_shadow::draw(
                                canvas,
                                &RoundedShadowDraw {
                                    bounds: shape,
                                    radius: 5.0,
                                    blur: 8.0,
                                    color,
                                    inner,
                                    draw_bounds,
                                }
                            ));
                            assert_eq!(canvas.save_count(), saved);
                        } else {
                            let mut paint = skia_safe::Paint::default();
                            paint.set_shader(
                                triangular_shadow::shader(shape, 5.0, 8.0, color, inner).unwrap(),
                            );
                            jian_skia::raster_sampling::configure_framebuffer_blend(&mut paint)
                                .unwrap();
                            canvas.draw_rect(draw_bounds, &paint);
                        }
                    }
                    assert_eq!(
                        bytes(&mut actual),
                        bytes(&mut expected),
                        "angle{angle} alpha{alpha} inner{inner} transparent{transparent}"
                    );
                }
            }
        }
    }
}
