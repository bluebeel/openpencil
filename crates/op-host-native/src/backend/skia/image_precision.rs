//! Keep fractional color values while constructing minification mip levels.

pub(super) fn minification_source(
    image: &skia_safe::Image,
    canvas: &skia_safe::Canvas,
    image_to_local: [f32; 6],
) -> skia_safe::Image {
    if !is_minified(canvas, image_to_local)
        || matches!(
            image.color_type(),
            skia_safe::ColorType::RGBAF16 | skia_safe::ColorType::RGBAF32
        )
    {
        return image.clone();
    }
    // Skia's 8-bit CPU mip chain rounds at each level, accumulating a dark
    // bias in deep reductions. Convert only this draw's source, preserving
    // the original color space and the existing 8-bit raster-cache budget.
    // The snapshot shares the temporary surface's allocation. Its lifetime
    // belongs to this draw; it is never installed in the persistent LRU.
    let info = skia_safe::ImageInfo::new(
        (image.width(), image.height()),
        skia_safe::ColorType::RGBAF16,
        skia_safe::AlphaType::Premul,
        image.color_space(),
    );
    let Some(mut surface) = skia_safe::surfaces::raster(&info, None, None) else {
        return image.clone();
    };
    surface.canvas().draw_image(image, (0.0, 0.0), None);
    surface.image_snapshot()
}

fn is_minified(canvas: &skia_safe::Canvas, local: [f32; 6]) -> bool {
    let device = canvas.local_to_device_as_3x3();
    let x = device.map_vector((local[0], local[3]));
    let y = device.map_vector((local[1], local[4]));
    let (xx, xy, yx, yy) = (x.x as f64, x.y as f64, y.x as f64, y.y as f64);
    let trace = xx * xx + xy * xy + yx * yx + yy * yy;
    let det = xx * yy - xy * yx;
    // The smaller singular value accounts for zoom, rotation and skew,
    // including affine crops whose source axes do not match node axes.
    let largest = (trace + (trace * trace - 4.0 * det * det).max(0.0).sqrt()) * 0.5;
    let squared_scale = det * det / largest;
    squared_scale.is_finite() && squared_scale > 0.0 && squared_scale < 1.0 - 1e-6
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precision_selection_respects_device_zoom_and_affine_minification() {
        let mut surface = skia_safe::surfaces::raster_n32_premul((16, 16)).unwrap();
        let canvas = surface.canvas();
        assert!(!is_minified(canvas, [1.0, 0.0, 0.0, 0.0, 1.0, 0.0]));
        assert!(!is_minified(canvas, [2.0, 0.0, 0.0, 0.0, 2.0, 0.0]));
        assert!(is_minified(canvas, [0.5, 0.0, 0.0, 0.0, 2.0, 0.0]));
        assert!(is_minified(canvas, [1.0, 1.0, 0.0, 0.0, 1.0, 0.0]));
        canvas.scale((0.5, 0.5));
        assert!(is_minified(canvas, [1.0, 0.0, 0.0, 0.0, 1.0, 0.0]));
    }
}
