//! Rounded shadow forwarding kept separate from the frame painter spine.
macro_rules! frame_shadow_methods {
    () => {
        fn shadow_offset_in_local_space(&self, offset: Point2D) -> Point2D {
            let current = self.canvas.local_to_device_as_3x3();
            if !current.is_finite()
                || !self.shadow_basis.is_finite()
                || current.has_perspective()
                || self.shadow_basis.has_perspective()
            {
                return offset;
            }
            if current.scale_x() == self.shadow_basis.scale_x()
                && current.scale_y() == self.shadow_basis.scale_y()
                && current.skew_x() == self.shadow_basis.skew_x()
                && current.skew_y() == self.shadow_basis.skew_y()
            {
                return offset;
            }
            let Some(inverse) = current.invert() else {
                return offset;
            };
            let device = self.shadow_basis.map_vector((offset.x, offset.y));
            let local = inverse.map_vector(device);
            if local.x.is_finite() && local.y.is_finite() {
                Point2D::new(local.x, local.y)
            } else {
                offset
            }
        }

        fn fill_round_rect_shadow(
            &mut self,
            rect: Rect,
            radius: f32,
            shadow: &op_editor_ui::RoundRectShadowOptions,
        ) -> bool {
            self.inner
                .fill_round_rect_shadow(self.canvas, rect, radius, shadow)
        }

        fn fill_drop_shadow(&mut self, rect: Rect, radius: f32, blur: f32, color: Color) {
            self.inner
                .fill_drop_shadow(self.canvas, rect, radius, blur, color);
        }
        fn fill_round_rect_shadow_per_corner(
            &mut self,
            rect: Rect,
            radii: [f32; 4],
            shadow: &op_editor_ui::RoundRectShadowOptions,
        ) -> bool {
            self.inner
                .fill_round_rect_shadow_per_corner(self.canvas, rect, radii, shadow)
        }
    };
}
pub(super) use frame_shadow_methods;
