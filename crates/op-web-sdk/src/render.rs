//! Each SDK Viewer owns its own CanvasKit session and idle rAF pump.
//! Pending payloads and DirtyState remain outside the paint-held RefCell.
//! Closures retain only Weak handles: idle detach/Drop needs no callback to
//! break a cycle, and one Viewer cannot replace or repaint another Viewer.
use crate::{
    dirty_state::{DirtyState, FailureAction},
    Viewer,
};
use op_editor_core::Viewport as DocViewport;
use op_editor_ui::{theme::Theme, RenderBackend};
use op_host_web::canvaskit::{init_backend, CanvasKitBackend};
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    sync::Arc,
};
use wasm_bindgen::{closure::Closure, prelude::*, JsCast};
type Scene = Arc<op_editor_ui::layout_scene::LayoutScene>;
#[path = "render/font_ready.rs"]
mod font_ready;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = error)]
    fn js_console_error(message: &str);
}

#[derive(Default)]
pub(crate) struct RenderSlot {
    attachment: RefCell<Option<Rc<()>>>,
    current: RefCell<Option<Session>>,
}
struct Session {
    shared: Rc<RenderShared>,
    pump: Rc<PumpHandle>,
}
#[derive(Clone, Copy, PartialEq)]
struct DisplaySize {
    width: u32,
    height: u32,
    dpr: f32,
}
impl DisplaySize {
    fn read(canvas: &web_sys::HtmlCanvasElement, window: &web_sys::Window) -> Self {
        let scale = window.device_pixel_ratio() as f32;
        Self {
            width: canvas.client_width().max(1) as u32,
            height: canvas.client_height().max(1) as u32,
            dpr: if scale.is_finite() && scale > 0.0 {
                scale.max(1.0)
            } else {
                1.0
            },
        }
    }
}
struct RenderShared {
    active: Cell<bool>,
    state: DirtyState,
    pending_scene: Cell<Option<Option<Scene>>>,
    pending_viewport: Cell<Option<DocViewport>>,
    pending_size: Cell<Option<DisplaySize>>,
    requested_size: Cell<DisplaySize>,
    canvas: web_sys::HtmlCanvasElement,
    font_ready: RefCell<Option<font_ready::Listener>>,
    pump: Weak<PumpHandle>,
    inner: RefCell<RenderInner>,
}
struct RenderInner {
    backend: CanvasKitBackend,
    logical_w: f32,
    logical_h: f32,
    scene: Option<Scene>,
    viewport: DocViewport,
}
impl Drop for RenderInner {
    fn drop(&mut self) {
        // This SDK bridge has no external measurement owners. Destruction can
        // wait for a reentrant paint borrow, but never disposes a new session.
        self.backend.dispose();
    }
}
struct PumpHandle {
    closure: RefCell<Option<Closure<dyn FnMut()>>>,
    raf_id: Cell<Option<i32>>,
    requesting: Cell<bool>,
}
impl PumpHandle {
    fn cancel(&self) {
        if let Some(id) = self.raf_id.take() {
            if let Some(window) = web_sys::window() {
                let _ = window.cancel_animation_frame(id);
            }
        }
    }
}
impl Drop for PumpHandle {
    fn drop(&mut self) {
        self.cancel();
        // Closure captures Weak only; its automatic drop releases the JS
        // callback even when clean/idle. An executing callback pins this Rc.
    }
}
impl RenderSlot {
    fn begin_attach(&self) -> Rc<()> {
        self.detach();
        let token = Rc::new(());
        self.attachment.replace(Some(Rc::clone(&token)));
        token
    }
    fn matches(&self, token: &Rc<()>) -> bool {
        self.attachment
            .borrow()
            .as_ref()
            .is_some_and(|current| Rc::ptr_eq(current, token))
    }
    fn shared(&self) -> Option<Rc<RenderShared>> {
        self.current
            .borrow()
            .as_ref()
            .map(|session| Rc::clone(&session.shared))
    }
    fn detach(&self) {
        self.attachment.borrow_mut().take();
        // No slot borrow crosses cancellation or a JS/backend destructor.
        let previous = self.current.borrow_mut().take();
        if let Some(session) = previous {
            session.shared.active.set(false);
            session.shared.state.cancel_schedule();
            session.pump.cancel();
            drop(session);
        }
    }
}
impl Drop for RenderSlot {
    fn drop(&mut self) {
        self.detach();
    }
}

#[wasm_bindgen]
impl Viewer {
    /// Reattach only this Viewer. A superseded async init cannot publish a
    /// stale surface after detach or a later attach on the same Viewer.
    pub async fn attach_canvas(&self, canvas_id: String) -> Result<(), JsValue> {
        let token = self.render.begin_attach();
        console_error_panic_hook::set_once();
        let window =
            web_sys::window().ok_or_else(|| JsValue::from_str("attach_canvas: no window"))?;
        let document = window
            .document()
            .ok_or_else(|| JsValue::from_str("attach_canvas: no document"))?;
        let canvas: web_sys::HtmlCanvasElement = document
            .get_element_by_id(&canvas_id)
            .ok_or_else(|| JsValue::from_str("attach_canvas: canvas not found"))?
            .dyn_into()
            .map_err(|_| JsValue::from_str("attach_canvas: element is not a <canvas>"))?;
        let initial_size = DisplaySize::read(&canvas, &window);
        let mut backend = init_backend(
            &canvas_id,
            initial_size.dpr,
            initial_size.width,
            initial_size.height,
        )
        .await?;
        if !self.render.matches(&token) {
            backend.dispose();
            return Err(JsValue::from_str("attach_canvas: attachment superseded"));
        }
        // A resize/DPR change can happen while CanvasKit is loading, before
        // a ResizeObserver has a live session to notify. Resample once here.
        let size = DisplaySize::read(&canvas, &window);
        if size != initial_size {
            backend.resize_for_display(size.width, size.height, size.dpr);
        }
        let pump = Rc::new(PumpHandle {
            closure: RefCell::new(None),
            raf_id: Cell::new(None),
            requesting: Cell::new(false),
        });
        let shared = Rc::new(RenderShared {
            active: Cell::new(true),
            state: DirtyState::new_dirty(),
            pending_scene: Cell::new(None),
            pending_viewport: Cell::new(None),
            pending_size: Cell::new(None),
            requested_size: Cell::new(size),
            canvas,
            font_ready: RefCell::new(None),
            pump: Rc::downgrade(&pump),
            inner: RefCell::new(RenderInner {
                backend,
                logical_w: size.width as f32,
                logical_h: size.height as f32,
                scene: self.scene.clone(),
                viewport: self.viewport,
            }),
        });
        shared
            .font_ready
            .replace(Some(font_ready::Listener::install(&shared)?));
        self.render.current.replace(Some(Session {
            shared: Rc::clone(&shared),
            pump: Rc::clone(&pump),
        }));
        start_raf_pump(&shared, &pump);
        Ok(())
    }
    /// Notify this Viewer's existing session of a CSS-size or display-DPR
    /// change. Safe during paint: backend resize is deferred to the next frame.
    /// Returns false when unattached or unchanged and does not wake an idle
    /// canvas for duplicate ResizeObserver notifications.
    pub fn resize_canvas(&self) -> bool {
        let (Some(shared), Some(window)) = (self.render.shared(), web_sys::window()) else {
            return false;
        };
        let size = DisplaySize::read(&shared.canvas, &window);
        if size == shared.requested_size.get() {
            return false;
        }
        shared.requested_size.set(size);
        shared.pending_size.set(Some(size));
        mark_and_arm(&shared);
        true
    }
    /// Idle-safe teardown of this Viewer's session only.
    pub fn detach(&self) {
        self.render.detach();
    }
    pub fn mark_dirty(&self) {
        if let Some(shared) = self.render.shared() {
            mark_and_arm(&shared);
        }
    }
    /// Arc-only scene transfer; no deep LayoutScene clone on load/page changes.
    /// Pending state is separate from the RefCell borrowed by an active paint.
    pub fn push_scene(&self) {
        if let Some(shared) = self.render.shared() {
            shared.pending_scene.set(Some(self.scene.clone()));
            mark_and_arm(&shared);
        }
    }
}
pub(crate) fn push_viewport_to_render(viewer: &Viewer, viewport: DocViewport) {
    if let Some(shared) = viewer.render.shared() {
        shared.pending_viewport.set(Some(viewport));
        mark_and_arm(&shared);
    }
}
fn mark_and_arm(shared: &RenderShared) {
    if shared.active.get() && shared.state.mark_dirty() {
        arm_pump(shared);
    }
}
fn arm_pump(shared: &RenderShared) {
    let Some(pump) = shared.pump.upgrade() else {
        return;
    };
    // A reentrant mark while requestAnimationFrame itself is executing still
    // queues its generation/payload. The outer request owns scheduling/retry;
    // do not create a duplicate callback or charge a false paint failure.
    if pump.requesting.get() {
        return;
    }
    while shared.active.get() {
        if request_pump_frame(shared, &pump) {
            shared.state.commit_arm();
            return;
        }
        if !shared.active.get() {
            return;
        }
        if shared.state.record_failure() == FailureAction::Stop {
            js_console_error("op-web-sdk: requestAnimationFrame failed repeatedly; render pump stopped. The next scene or viewport update will retry.");
            return;
        }
    }
}
fn request_pump_frame(shared: &RenderShared, pump: &PumpHandle) -> bool {
    let closure = pump.closure.borrow();
    let Some(callback) = closure.as_ref() else {
        return false;
    };
    let Some(window) = web_sys::window() else {
        return false;
    };
    struct Reset<'a>(&'a Cell<bool>);
    impl Drop for Reset<'_> {
        fn drop(&mut self) {
            self.0.set(false);
        }
    }
    pump.requesting.set(true);
    let _request = Reset(&pump.requesting);
    match window.request_animation_frame(callback.as_ref().unchecked_ref()) {
        Ok(id) if shared.active.get() => {
            pump.raf_id.set(Some(id));
            true
        }
        Ok(id) => {
            let _ = window.cancel_animation_frame(id);
            false
        }
        Err(_) => false,
    }
}
fn start_raf_pump(shared: &Rc<RenderShared>, pump: &Rc<PumpHandle>) {
    let weak_shared = Rc::downgrade(shared);
    let weak_pump = Rc::downgrade(pump);
    *pump.closure.borrow_mut() = Some(Closure::wrap(Box::new(move || {
        let (Some(shared), Some(pump)) = (weak_shared.upgrade(), weak_pump.upgrade()) else {
            return;
        };
        pump.raf_id.set(None);
        if !shared.active.get() {
            return;
        }
        let Some(generation) = shared.state.begin_paint() else {
            return;
        };
        if paint_frame(&shared) {
            if shared.active.get() && shared.state.finish_paint_success(generation) {
                arm_pump(&shared);
            }
        } else if shared.active.get() {
            match shared.state.record_failure() {
                FailureAction::Rearm => arm_pump(&shared),
                FailureAction::Stop => js_console_error("op-web-sdk: repaint failed repeatedly; render pump stopped. The next scene or viewport update will retry."),
            }
        }
    }) as Box<dyn FnMut()>));
    arm_pump(shared);
}

fn paint_frame(shared: &RenderShared) -> bool {
    // The pump is the only mutable borrower and this is single-threaded, so
    // a conflict here is unexpected; treat it as a failed (retryable) paint
    // rather than falsely advancing `painted_gen` past unpainted content.
    let Ok(mut b) = shared.inner.try_borrow_mut() else {
        return false;
    };
    // Apply any pending push_scene / push_viewport_to_render payload
    // BEFORE reading state to paint (F3 fix). A push that landed while a
    // previous frame's paint held this same borrow could only queue into
    // `pending_scene` / `pending_viewport` (see those functions); draining
    // it here — at the very start of every paint attempt, unconditionally
    // — is what makes the dirty generation bumped at push time honest:
    // the generation is never consumed by a frame that painted stale
    // (or missing) payload.
    if let Some(size) = shared.pending_size.take() {
        b.backend
            .resize_for_display(size.width, size.height, size.dpr);
        b.logical_w = size.width as f32;
        b.logical_h = size.height as f32;
    }
    if let Some(pending_scene) = shared.pending_scene.take() {
        b.scene = pending_scene;
    }
    if let Some(pending_viewport) = shared.pending_viewport.take() {
        b.viewport = pending_viewport;
    }
    // No scene yet (document not loaded) — nothing to paint counts as
    // success: the generation is consumed, and `push_scene` re-arms when
    // content arrives, so the pump never polls for a missing scene.
    let Some(scene_rc) = b.scene.clone() else {
        return true;
    };
    // Rc clone (refcount bump), not a deep LayoutScene clone. Cloning the
    // handle — rather than reading `b.scene` directly — avoids holding an
    // aliasing borrow across the mutable `b.backend` paint calls below.
    let vp = b.viewport;
    let w = b.logical_w;
    let h = b.logical_h;
    // A scene paint records encoded-image decode work into the shared image
    // runtime. Drain work left by the previous frame before painting, then
    // drain again afterwards so this frame's newly requested images are
    // decoded and can be shown by the next dirty repaint. The full web host
    // does the same at the start of `CkInner::repaint`; the standalone SDK
    // owns its own pump and therefore must service that queue itself.
    b.backend.drain_pending_decodes(2);
    b.backend.begin_frame();
    crate::viewer_host::paint_scene(&mut b.backend, &scene_rc, vp, Theme::dark(), w, h);
    b.backend.end_frame();
    if b.backend.drain_pending_decodes(2) > 0 {
        mark_and_arm(shared);
    }
    true
}
