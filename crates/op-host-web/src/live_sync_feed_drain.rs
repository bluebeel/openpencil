//! Read a bounded since-cursor prefix while the live document stays untouched.
//! Each page yields to the browser; one final transaction/paint acknowledges it.

use super::{current_pair, SharedSync};
use crate::{live_sync, repaint_ctx::RepaintContext};
use op_editor_core::web_sync::CommittedMetadata;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use wasm_bindgen::{closure::Closure, JsCast};

const MAX_PAGES: usize = 16;
const DRAIN_WINDOW_MS: f64 = 250.0;

struct Drain<C: RepaintContext> {
    inner: Rc<RefCell<C>>,
    sync: SharedSync,
    busy: Rc<Cell<bool>>,
    selection_key: Rc<RefCell<Option<String>>>,
    base: String,
    issued_cursor: String,
    issued_version: u64,
    issued_pair: (u64, u64),
    epoch: u64,
    started: f64,
    pages: usize,
    pending: Option<CommittedMetadata>,
    done: bool,
}
impl<C: RepaintContext> Drain<C> {
    fn same_source(&self) -> bool {
        if self.epoch != crate::identity_epoch::epoch() || super::super::auth_is_invalid() {
            return false;
        }
        let Ok(sync) = self.sync.try_borrow() else {
            return false;
        };
        let Ok(context) = self.inner.try_borrow() else {
            return false;
        };
        sync.client.change_cursor() == Some(self.issued_cursor.as_str())
            && sync.client.applied_version() == self.issued_version
            && current_pair(&*context) == self.issued_pair
            && sync.gate.pull_allowed(self.issued_pair)
    }
    fn stop(&mut self) {
        self.done = true;
        self.pending = None;
        self.busy.set(false);
    }
}

pub(super) fn start<C: RepaintContext + 'static>(
    inner: &Rc<RefCell<C>>,
    base: &str,
    sync: &SharedSync,
    busy: &Rc<Cell<bool>>,
    selection_key: &Rc<RefCell<Option<String>>>,
) -> bool {
    let Some((cursor, version)) = sync.try_borrow().ok().and_then(|sync| {
        sync.client
            .change_cursor()
            .map(|cursor| (cursor.to_owned(), sync.client.applied_version()))
    }) else {
        return false;
    };
    let Ok(context) = inner.try_borrow() else {
        return true;
    };
    let pair = current_pair(&*context);
    drop(context);
    let state = Rc::new(RefCell::new(Drain {
        inner: Rc::clone(inner),
        sync: Rc::clone(sync),
        busy: Rc::clone(busy),
        selection_key: Rc::clone(selection_key),
        base: base.to_owned(),
        issued_cursor: cursor,
        issued_version: version,
        issued_pair: pair,
        epoch: crate::identity_epoch::epoch(),
        started: js_sys::Date::now(),
        pages: 0,
        pending: None,
        done: false,
    }));
    if !state.borrow().same_source() {
        return true;
    }
    busy.set(true);
    request(state);
    true
}

fn request<C: RepaintContext + 'static>(state: Rc<RefCell<Drain<C>>>) {
    let collection_finished = {
        let drain = state.borrow();
        let elapsed = js_sys::Date::now() - drain.started;
        drain.pending.is_some()
            && (drain.pages >= MAX_PAGES || !(0.0..DRAIN_WINDOW_MS).contains(&elapsed))
    };
    if collection_finished {
        finish(state);
        return;
    }
    let url = {
        let mut drain = state.borrow_mut();
        if drain.done {
            return;
        }
        if !drain.same_source() {
            drain.stop();
            return;
        }
        let cursor = drain
            .pending
            .as_ref()
            .map_or(&drain.issued_cursor, |pending| &pending.cursor);
        format!("{}/api/mcp/changes?since={cursor}", drain.base)
    };
    let callback_state = Rc::clone(&state);
    let response: Rc<dyn Fn(u16, String)> = Rc::new(move |status, body| {
        receive(Rc::clone(&callback_state), status, body);
    });
    if !live_sync::get_with_status(&url, response) {
        state.borrow_mut().stop();
    }
}

fn receive<C: RepaintContext + 'static>(state: Rc<RefCell<Drain<C>>>, status: u16, body: String) {
    let next = {
        let mut drain = state.borrow_mut();
        if drain.done {
            return;
        }
        if !drain.same_source() {
            drain.stop();
            return;
        }
        if matches!(status, 401 | 403) {
            super::super::note_auth_invalid();
            drain.stop();
            return;
        }
        if !matches!(status, 200 | 404 | 409) {
            // A transient/fenced read does not acknowledge any collected data.
            drain.stop();
            return;
        }
        let version = drain
            .pending
            .as_ref()
            .map_or(drain.issued_version, |pending| pending.version);
        if status == 200 {
            CommittedMetadata::parse(&body, version).ok()
        } else {
            None
        }
    };
    // Drop the encoded XHR response before starting another page. Only parsed
    // bounded state is retained; no full Doc or snapshots are accumulated.
    drop(body);
    let Some(next) = next else {
        resync(state);
        return;
    };
    let continue_drain = {
        let mut drain = state.borrow_mut();
        if drain.done || !drain.same_source() {
            drain.stop();
            return;
        }
        let authority_changed = drain
            .sync
            .borrow()
            .client
            .change_authority_differs(&next.cursor);
        if authority_changed {
            drop(drain);
            resync(state);
            return;
        }
        drain.pages += 1;
        let appended = if let Some(pending) = drain.pending.as_mut() {
            pending.append_page(next)
        } else {
            drain.pending = Some(next);
            Ok(true)
        };
        let appended = match appended {
            Ok(appended) => appended,
            Err(_) => {
                drop(drain);
                resync(state);
                return;
            }
        };
        let elapsed = js_sys::Date::now() - drain.started;
        appended
            && drain.pages < MAX_PAGES
            && (0.0..DRAIN_WINDOW_MS).contains(&elapsed)
            && drain
                .pending
                .as_ref()
                .is_some_and(CommittedMetadata::can_drain_more)
    };
    // Also yield before the final apply/paint, separating it from JSON decode.
    // The final continuation rechecks authority and local-edit identity again.
    if !defer(Rc::clone(&state), continue_drain) {
        finish(state);
    }
}

fn defer<C: RepaintContext + 'static>(state: Rc<RefCell<Drain<C>>>, more: bool) -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    let callback = Closure::<dyn FnMut()>::once_into_js(move || {
        if more {
            request(state);
        } else {
            finish(state);
        }
    });
    window
        .set_timeout_with_callback_and_timeout_and_arguments_0(callback.unchecked_ref(), 0)
        .is_ok()
}

fn finish<C: RepaintContext + 'static>(state: Rc<RefCell<Drain<C>>>) {
    let (pending, inner, sync, busy, selection, pair) = {
        let mut drain = state.borrow_mut();
        if drain.done {
            return;
        }
        if !drain.same_source() {
            drain.stop();
            return;
        }
        let Some(pending) = drain.pending.take() else {
            drain.stop();
            return;
        };
        drain.done = true;
        (
            pending,
            Rc::clone(&drain.inner),
            Rc::clone(&drain.sync),
            Rc::clone(&drain.busy),
            Rc::clone(&drain.selection_key),
            drain.issued_pair,
        )
    };
    let structural = pending.changes_structure;
    if super::apply_metadata(&inner, &sync, pending, pair) {
        if structural {
            *selection.borrow_mut() = None;
        }
        busy.set(false);
    } else {
        let still_current = state.borrow().same_source();
        if still_current {
            // Apply refused before mutation or rolled back paint/cursor failure.
            resync(state);
        } else {
            busy.set(false);
        }
    }
}

fn resync<C: RepaintContext + 'static>(state: Rc<RefCell<Drain<C>>>) {
    let (inner, base, sync, busy, selection) = {
        let mut drain = state.borrow_mut();
        if !drain.same_source() {
            drain.stop();
            return;
        }
        drain.done = true;
        drain.pending = None;
        (
            Rc::clone(&drain.inner),
            drain.base.clone(),
            Rc::clone(&drain.sync),
            Rc::clone(&drain.busy),
            Rc::clone(&drain.selection_key),
        )
    };
    sync.borrow_mut().client.require_change_resync();
    super::fetch_snapshot(&inner, &base, &sync, &busy, &selection);
}
