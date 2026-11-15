//! Transfer the actual path's lexical packet to its synchronous JS draw only.
use crate::canvaskit::bindings::OpCk;
use wasm_bindgen::JsValue;
pub(super) struct Scope<'a> {
    ck: &'a OpCk,
    old: Option<JsValue>,
}
impl<'a> Scope<'a> {
    pub(super) fn new(ck: &'a OpCk) -> Self {
        let old = op_editor_ui::svg_path_coordinates::current().map(|coordinates| {
            let values = js_sys::Array::new();
            for value in coordinates {
                values.push(&JsValue::from_f64(value as f64));
            }
            ck.set_svg_path_view_box(values.as_ref())
        });
        Self { ck, old }
    }
}
impl Drop for Scope<'_> {
    fn drop(&mut self) {
        if let Some(old) = &self.old {
            self.ck.set_svg_path_view_box(old);
        }
    }
}
