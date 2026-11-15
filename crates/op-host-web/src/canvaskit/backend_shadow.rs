//! Rounded shadow methods split from the platform painter spine.
macro_rules! shadow_methods {
    () => {
        fn fill_drop_shadow(&mut self, rect: Rect, radius: f32, blur: f32, color: Color) {
            let shadow = op_editor_ui::RoundRectShadowOptions {
                offset: Point2D::ZERO,
                blur,
                spread: 0.0,
                color,
                kernel: op_editor_ui::ShadowBlurKernel::Gaussian,
                inner: false,
            };
            if !self.fill_round_rect_shadow(rect, radius, &shadow) {
                self.fill_round_rect(rect, radius, color);
            }
        }
        fn fill_round_rect_shadow(
            &mut self,
            rect: Rect,
            radius: f32,
            shadow: &op_editor_ui::RoundRectShadowOptions,
        ) -> bool {
            self.ck.fill_round_rect_shadow(
                rect.origin.x,
                rect.origin.y,
                rect.size.x,
                rect.size.y,
                radius,
                shadow.offset.x,
                shadow.offset.y,
                shadow.blur,
                shadow.spread,
                shadow.color.r,
                shadow.color.g,
                shadow.color.b,
                shadow.color.a,
                shadow.inner,
                u32::from(shadow.kernel == op_editor_ui::ShadowBlurKernel::Triangular),
            )
        }

        fn fill_round_rect_shadow_per_corner(
            &mut self,
            rect: Rect,
            radii: [f32; 4],
            shadow: &op_editor_ui::RoundRectShadowOptions,
        ) -> bool {
            self.ck.fill_round_rect_shadow_per_corner(
                rect.origin.x,
                rect.origin.y,
                rect.size.x,
                rect.size.y,
                radii[0],
                radii[1],
                radii[2],
                radii[3],
                shadow.offset.x,
                shadow.offset.y,
                shadow.blur,
                shadow.spread,
                shadow.color.r,
                shadow.color.g,
                shadow.color.b,
                shadow.color.a,
                shadow.inner,
                u32::from(shadow.kernel == op_editor_ui::ShadowBlurKernel::Triangular),
            )
        }
    };
}
pub(super) use shadow_methods;
