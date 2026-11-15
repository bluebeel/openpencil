//! Explicit PDF drawing target. Skia's PDF clip stack serializes geometric
//! clips; shader clip elements become an unbounded path, not their coverage.
//! Keep the authored outline/clip geometry as vectors on that target instead
//! of sending device-sampled raster masks through an unsupported clip API.
use super::NativeBackend;
use std::cell::Cell;
thread_local! { static PDF: Cell<bool> = const { Cell::new(false) }; }

pub(super) fn is_pdf() -> bool {
    PDF.with(Cell::get)
}

impl NativeBackend {
    /// Scope the existing native painter to a PDF canvas. This changes only
    /// target representation: positioned glyph paths and rounded geometric
    /// clips stay vectors. Shaping, contour construction, CTM, paint ordering,
    /// source color/opacity and blend/layer semantics are left to their owners.
    pub fn with_pdf_document<R>(paint: impl FnOnce() -> R) -> R {
        struct Restore(bool);
        impl Drop for Restore {
            fn drop(&mut self) {
                PDF.with(|scope| scope.set(self.0));
            }
        }
        let _restore = Restore(PDF.with(|scope| scope.replace(true)));
        paint()
    }
}
