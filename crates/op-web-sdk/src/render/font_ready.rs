//! One owner-targeted font-ready wake; no global Viewer/render slot or cycle.
use super::{mark_and_arm, RenderShared};
use std::rc::Rc;
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
pub(super) struct Listener {
    canvas: web_sys::HtmlCanvasElement,
    callback: Closure<dyn FnMut(web_sys::Event)>,
}
impl Listener {
    pub(super) fn install(owner: &Rc<RenderShared>) -> Result<Self, JsValue> {
        let weak = Rc::downgrade(owner);
        let callback = Closure::new(move |_event: web_sys::Event| {
            if let Some(owner) = weak.upgrade() {
                mark_and_arm(&owner);
            }
        });
        owner.canvas.add_event_listener_with_callback(
            "openpencil-font-ready",
            callback.as_ref().unchecked_ref(),
        )?;
        Ok(Self {
            canvas: owner.canvas.clone(),
            callback,
        })
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        let _ = self.canvas.remove_event_listener_with_callback(
            "openpencil-font-ready",
            self.callback.as_ref().unchecked_ref(),
        );
    }
}
