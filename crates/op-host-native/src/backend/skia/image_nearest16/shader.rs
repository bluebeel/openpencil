//! Stratified point sampling with color and geometric coverage resolved together.

use skia_safe::{
    runtime_effect::ChildPtr, Canvas, Data, Image, Matrix, RRect, RuntimeEffect, Shader,
};

const SOURCE: &str = r#"
uniform shader image;
uniform float4 bounds;
uniform float4 topRadii;
uniform float4 bottomRadii;
uniform float4 deviceInverse;
uniform float4 imageInverse;
uniform float2 imageOffset;
uniform float2 sourceSize;
uniform float directRotationUv;
uniform float grid;
float ellipse(float2 p, float2 center, float2 radius) {
    float2 q = (p - center) / radius;
    return dot(q, q) <= 1.0 ? 1.0 : 0.0;
}
float inside(float2 p) {
    if (p.x < bounds.x || p.y < bounds.y || p.x >= bounds.z || p.y >= bounds.w) return 0.0;
    if (p.x < bounds.x + topRadii.x && p.y < bounds.y + topRadii.y)
        return ellipse(p, bounds.xy + topRadii.xy, topRadii.xy);
    if (p.x > bounds.z - topRadii.z && p.y < bounds.y + topRadii.w)
        return ellipse(p, float2(bounds.z - topRadii.z, bounds.y + topRadii.w), topRadii.zw);
    if (p.x > bounds.z - bottomRadii.x && p.y > bounds.w - bottomRadii.y)
        return ellipse(p, bounds.zw - bottomRadii.xy, bottomRadii.xy);
    if (p.x < bounds.x + bottomRadii.z && p.y > bounds.w - bottomRadii.w)
        return ellipse(p, float2(bounds.x + bottomRadii.z, bounds.w - bottomRadii.w), bottomRadii.zw);
    return 1.0;
}
float quantizeByte(float value) {
    float scaled = clamp(value, 0.0, 1.0) * 255.0;
    float base = floor(scaled);
    float fraction = scaled - base;
    if (fraction > .5 || (fraction == .5 && mod(base, 2.0) != 0.0)) base += 1.0;
    return base;
}
float4 storeRGBA8(float4 value) {
    float4 bytes = float4(quantizeByte(value.r), quantizeByte(value.g),
                          quantizeByte(value.b), quantizeByte(value.a));
    // Framebuffer destination loads expand UNORM8 to UNORM16 before float
    // blending. Keep the integer expansion: reciprocal255 has different ties.
    return float4(int4(bytes) * 257) * (1.0 / 65535.0);
}
float4 sampleBytes(float2 p, float2 offset) {
    float2 q = p + float2(deviceInverse.x * offset.x + deviceInverse.y * offset.y,
                          deviceInverse.z * offset.x + deviceInverse.w * offset.y);
    float2 uv = float2(imageInverse.x * q.x + imageInverse.y * q.y,
                       imageInverse.z * q.x + imageInverse.w * q.y) + imageOffset;
    float2 imagePoint = q;
    if (directRotationUv != 0.0) {
        float2 unit = (q - bounds.xy) / (bounds.zw - bounds.xy);
        uv = float2(imageInverse.x * unit.x + imageInverse.y * unit.y,
                    imageInverse.z * unit.x + imageInverse.w * unit.y) + imageOffset;
        imagePoint = uv * sourceSize;
    }
    if (inside(q) == 0.0 || (directRotationUv == 0.0 && (any(lessThan(uv, float2(0.0))) || any(greaterThan(uv, float2(1.0)))))) return float4(0.0);
    // The source pass writes the point-sampled premultiplied color to RGBA8
    // before its 2x resolve. Recover those byte values before averaging.
    float4 color = float4(image.eval(imagePoint));
    return float4(quantizeByte(color.r), quantizeByte(color.g),
                  quantizeByte(color.b), quantizeByte(color.a));
}
float4 resolvePass(float2 p, float2 jitter) {
    float4 bytes = float4(0.0);
    for (int y = 0; y < 2; ++y) {
        for (int x = 0; x < 2; ++x) {
            if (float(x) < grid && float(y) < grid) {
                float2 tile = float2(float(x), float(y));
                bytes += sampleBytes(p, (tile + jitter) / grid - .5);
            }
        }
    }
    return (bytes / (grid * grid)) / 255.0;
}
half4 main(float2 p) {
    if (grid == 0.0) return half4(sampleBytes(p, float2(0.0)) / 255.0);
    // Resolve each camera jitter before blending into an RGBA8 running
    // average. The order matters because every atlas write is quantized.
    float4 atlas = storeRGBA8(resolvePass(p, float2(.625, .875)));
    atlas = storeRGBA8(resolvePass(p, float2(.375, .125)) * .5 + atlas * .5);
    float third = 1.0 / 3.0;
    atlas = storeRGBA8(resolvePass(p, float2(.125, .625)) * third + atlas * (1.0 - third));
    atlas = storeRGBA8(resolvePass(p, float2(.875, .375)) * .25 + atlas * .75);
    return half4(atlas);
}
"#;

thread_local! {
    static EFFECT: Option<RuntimeEffect> = RuntimeEffect::make_for_shader(SOURCE, None).ok();
}

#[cfg(test)]
pub(super) fn make(
    canvas: &Canvas,
    image: &Image,
    local: &Matrix,
    shape: &RRect,
    grid: f32,
) -> Option<Shader> {
    make_with_rotation(canvas, image, local, None, shape, grid)
}

pub(super) fn make_with_rotation(
    canvas: &Canvas,
    image: &Image,
    local: &Matrix,
    rotation_uv: Option<[f32; 6]>,
    shape: &RRect,
    grid: f32,
) -> Option<Shader> {
    let device = canvas.local_to_device_as_3x3();
    if device.has_perspective() || local.has_perspective() {
        return None;
    }
    let inverse = device.invert()?;
    let uv = local.invert()?;
    let child = image.to_shader(
        (skia_safe::TileMode::Clamp, skia_safe::TileMode::Clamp),
        skia_safe::SamplingOptions::default(),
        rotation_uv.is_none().then_some(local),
    )?;
    let rect = shape.rect();
    let radii = shape.radii_ref();
    let w = image.width() as f32;
    let h = image.height() as f32;
    let uv_values = rotation_uv.unwrap_or([
        uv.scale_x() / w,
        uv.skew_x() / w,
        uv.translate_x() / w,
        uv.skew_y() / h,
        uv.scale_y() / h,
        uv.translate_y() / h,
    ]);
    let uniforms = [
        rect.left,
        rect.top,
        rect.right,
        rect.bottom,
        radii[0].x,
        radii[0].y,
        radii[1].x,
        radii[1].y,
        radii[2].x,
        radii[2].y,
        radii[3].x,
        radii[3].y,
        inverse.scale_x(),
        inverse.skew_x(),
        inverse.skew_y(),
        inverse.scale_y(),
        uv_values[0],
        uv_values[1],
        uv_values[3],
        uv_values[4],
        uv_values[2],
        uv_values[5],
        w,
        h,
        if rotation_uv.is_some() { 1.0 } else { 0.0 },
        grid,
    ];
    let bytes: Vec<u8> = uniforms.iter().flat_map(|v| v.to_ne_bytes()).collect();
    EFFECT.with(|effect| {
        let effect = effect.as_ref()?;
        if effect.uniform_size() != bytes.len() {
            return None;
        }
        effect.make_shader(Data::new_copy(&bytes), &[ChildPtr::Shader(child)], None)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_rotated_uv_preserves_point_texels_at_nonzero_origins_and_zoom() {
        let pixels: Vec<u8> = (0..16)
            .flat_map(|i| [(i % 4 * 50) as u8, (i / 4 * 50) as u8, 17, 255])
            .collect();
        let info = skia_safe::ImageInfo::new(
            (4, 4),
            skia_safe::ColorType::RGBA8888,
            skia_safe::AlphaType::Unpremul,
            None,
        );
        let image =
            skia_safe::images::raster_from_data(&info, Data::new_copy(&pixels), 16).unwrap();
        for (x, y, zoom) in [(0.0, 0.0, 1.0), (7.0, 11.0, 1.0), (2.0, 3.0, 2.0)] {
            let rect = op_editor_ui::Rect::xywh(x, y, 4.0, 4.0);
            let uv = super::super::super::image_rotation::source_fit([4.0, 4.0], [4.0, 4.0], 90.0)
                .unwrap();
            let values =
                super::super::super::image::figma_image_local_matrix(rect, 4.0, 4.0, uv).unwrap();
            let local = Matrix::new_all(
                values[0], values[1], values[2], values[3], values[4], values[5], 0.0, 0.0, 1.0,
            );
            let mut surface = skia_safe::surfaces::raster_n32_premul((40, 40)).unwrap();
            surface.canvas().scale((zoom, zoom));
            let shape = RRect::new_rect_xy(skia_safe::Rect::from_xywh(x, y, 4.0, 4.0), 0.0, 0.0);
            let shader =
                make_with_rotation(surface.canvas(), &image, &local, Some(uv), &shape, 0.0)
                    .unwrap();
            let mut paint = skia_safe::Paint::default();
            paint.set_shader(shader);
            surface.canvas().draw_rect(shape.rect(), &paint);
            let snapshot = surface.image_snapshot();
            let actual = snapshot.peek_pixels().unwrap();
            for py in 0..4 {
                for px in 0..4 {
                    let color = actual.get_color((
                        ((x + px as f32 + 0.5) * zoom) as i32,
                        ((y + py as f32 + 0.5) * zoom) as i32,
                    ));
                    assert_eq!(
                        [color.r(), color.g(), color.b(), color.a()],
                        [(py * 50) as u8, ((3 - px) * 50) as u8, 17, 255]
                    );
                }
            }
        }
    }

    #[test]
    fn framebuffer_normalization_preserves_even_byte_ties_during_blending() {
        // Four independent source groups resolve to16,45,30,30. The second
        // atlas write is exactly30.5 bytes: the UNORM16 destination load
        // keeps the ties-even result30, whereas a reciprocal255 load gives31.
        let groups = [[5, 7, 13, 15], [0, 2, 8, 10], [4, 6, 12, 14], [1, 3, 9, 11]];
        let mut pixels = vec![0_u8; 4 * 4 * 4];
        for (group, value) in groups.into_iter().zip([16, 45, 30, 30]) {
            for index in group {
                pixels[index * 4..index * 4 + 4].copy_from_slice(&[value, 80, 160, 255]);
            }
        }
        let info = skia_safe::ImageInfo::new(
            (4, 4),
            skia_safe::ColorType::RGBA8888,
            skia_safe::AlphaType::Unpremul,
            None,
        );
        let image =
            skia_safe::images::raster_from_data(&info, Data::new_copy(&pixels), 16).unwrap();
        let mut surface = skia_safe::surfaces::raster_n32_premul((1, 1)).unwrap();
        let rect = skia_safe::Rect::from_wh(1.0, 1.0);
        let shader = make(
            surface.canvas(),
            &image,
            &Matrix::scale((0.25, 0.25)),
            &RRect::new_rect_xy(rect, 0.0, 0.0),
            2.0,
        )
        .unwrap();
        let mut paint = skia_safe::Paint::default();
        paint.set_shader(shader);
        surface.canvas().draw_rect(rect, &paint);
        let pixel = surface
            .image_snapshot()
            .peek_pixels()
            .unwrap()
            .get_color((0, 0));
        assert_eq!(
            (pixel.r(), pixel.g(), pixel.b(), pixel.a()),
            (30, 80, 160, 255)
        );
    }

    #[test]
    fn atlas_writes_match_independent_captured_running_average_math() {
        let mut pixels: Vec<u8> = (0..16)
            .flat_map(|i| [((i * 37 + 11) % 256) as u8, 90, 170, 255])
            .collect();
        pixels[0] = 14;
        pixels[4] = 47;
        // These disjoint groups are the four camera jitters after the
        // 2x2 spatial resolve, in their observed submission order.
        let groups = [[5, 7, 13, 15], [0, 2, 8, 10], [4, 6, 12, 14], [1, 3, 9, 11]];
        let mut stored = 0.0_f32;
        for (group, weight) in groups.into_iter().zip([1.0_f32, 0.5, 1.0 / 3.0, 0.25]) {
            let incoming = group
                .into_iter()
                .map(|i| f32::from(pixels[i * 4]))
                .sum::<f32>()
                * (1.0_f32 / 1020.0);
            let destination = ((stored as u32 * 257) as f32) * (1.0_f32 / 65535.0);
            stored = ((incoming * weight + destination * (1.0 - weight)) * 255.0).round_ties_even();
        }
        let single_average =
            (pixels.chunks_exact(4).map(|p| f32::from(p[0])).sum::<f32>() / 16.0).round_ties_even();
        assert_ne!(
            stored, single_average,
            "fixture must distinguish atlas rounding from one final store"
        );
        let info = skia_safe::ImageInfo::new(
            (4, 4),
            skia_safe::ColorType::RGBA8888,
            skia_safe::AlphaType::Unpremul,
            None,
        );
        let image =
            skia_safe::images::raster_from_data(&info, Data::new_copy(&pixels), 16).unwrap();
        let mut surface = skia_safe::surfaces::raster_n32_premul((1, 1)).unwrap();
        let rect = skia_safe::Rect::from_wh(1.0, 1.0);
        let shader = make(
            surface.canvas(),
            &image,
            &Matrix::scale((0.25, 0.25)),
            &RRect::new_rect_xy(rect, 0.0, 0.0),
            2.0,
        )
        .unwrap();
        let mut paint = skia_safe::Paint::default();
        paint.set_shader(shader);
        surface.canvas().draw_rect(rect, &paint);
        assert_eq!(
            surface
                .image_snapshot()
                .peek_pixels()
                .unwrap()
                .get_color((0, 0))
                .r(),
            stored as u8
        );
    }

    #[test]
    fn nearest_samples_preserve_premultiplied_average_under_device_zoom() {
        let pixels: Vec<u8> = (0..16)
            .flat_map(|i| [i * 16, 128, 255 - i * 16, 255])
            .collect();
        let info = skia_safe::ImageInfo::new(
            (4, 4),
            skia_safe::ColorType::RGBA8888,
            skia_safe::AlphaType::Unpremul,
            None,
        );
        let image =
            skia_safe::images::raster_from_data(&info, Data::new_copy(&pixels), 16).unwrap();
        for zoom in [1.0, 2.0] {
            let mut surface = skia_safe::surfaces::raster_n32_premul((1, 1)).unwrap();
            let canvas = surface.canvas();
            canvas.scale((zoom, zoom));
            let side = 1.0 / zoom;
            let local = Matrix::scale((side / 4.0, side / 4.0));
            let shape = RRect::new_rect_xy(skia_safe::Rect::from_wh(side, side), 0.0, 0.0);
            let effect = RuntimeEffect::make_for_shader(SOURCE, None);
            assert!(effect.is_ok(), "{effect:?}");
            let shader = make(canvas, &image, &local, &shape, 2.0).unwrap();
            let mut paint = skia_safe::Paint::default();
            paint.set_shader(shader);
            canvas.draw_rect(skia_safe::Rect::from_wh(side, side), &paint);
            let pixel = surface
                .image_snapshot()
                .peek_pixels()
                .unwrap()
                .get_color((0, 0));
            assert!((119..=121).contains(&pixel.r()));
            assert!((127..=129).contains(&pixel.g()));
            assert!((134..=136).contains(&pixel.b()));
            assert_eq!(pixel.a(), 255);
        }
    }
}
