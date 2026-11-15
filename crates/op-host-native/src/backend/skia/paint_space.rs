//! A bounded field-coordinate scope decouples brush mapping from the rectangle
//! used to cover a stroke band. It is inactive for all legacy paint calls.

use super::NativeBackend;
use op_editor_ui::Rect;
const MAX_PAINT_SPACES: usize = 16;
impl NativeBackend {
    pub fn push_paint_space(&mut self, rect: Rect) {
        assert!(
            self.paint_spaces.len() < MAX_PAINT_SPACES,
            "paint coordinate nesting exceeds 16"
        );
        self.paint_spaces.push(rect);
    }
    pub fn pop_paint_space(&mut self) {
        assert!(
            self.paint_spaces.pop().is_some(),
            "unbalanced paint coordinate scope"
        );
    }
    pub(super) fn active_paint_rect(&self, fallback: Rect) -> Rect {
        self.paint_spaces.last().copied().unwrap_or(fallback)
    }
    pub(super) fn has_paint_space(&self) -> bool {
        !self.paint_spaces.is_empty()
    }
}
