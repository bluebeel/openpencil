//! Bounded incremental pulls for ordinary metadata edits. Unsupported structural
//! records explicitly use the complete snapshot path; no command replay.

use super::{current_pair, SharedSync};
use crate::{live_sync, repaint_ctx::RepaintContext};
use op_editor_core::web_sync::CommittedMetadata;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

pub(super) fn try_fetch<C: RepaintContext + 'static>(
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
    let issued_pair = current_pair(&*context);
    drop(context);
    if !sync.borrow().gate.pull_allowed(issued_pair) {
        return true;
    }
    let epoch = crate::identity_epoch::epoch();
    busy.set(true);
    let url = format!("{base}/api/mcp/changes?since={cursor}");
    let inner = Rc::clone(inner);
    let sync = Rc::clone(sync);
    let busy_done = Rc::clone(busy);
    let selection_key = Rc::clone(selection_key);
    let base = base.to_owned();
    let response: Rc<dyn Fn(u16, String)> = Rc::new(move |status, body| {
        if epoch != crate::identity_epoch::epoch()
            || sync.borrow().client.change_cursor() != Some(cursor.as_str())
        {
            busy_done.set(false);
            return;
        }
        if matches!(status, 401 | 403) {
            super::note_auth_invalid();
            busy_done.set(false);
            return;
        }
        if !matches!(status, 200 | 404 | 409) {
            busy_done.set(false); // Transient/fenced errors never become data.
            return;
        }
        let gate_open = inner.try_borrow().ok().is_some_and(|context| {
            current_pair(&*context) == issued_pair && sync.borrow().gate.pull_allowed(issued_pair)
        });
        if !gate_open {
            busy_done.set(false);
            return;
        }
        if status == 200 {
            if let Ok(batch) = CommittedMetadata::parse(&body, version) {
                if !sync.borrow().client.change_authority_differs(&batch.cursor)
                    && apply_metadata(&inner, &sync, batch, issued_pair)
                {
                    busy_done.set(false);
                    return;
                }
            }
        }
        // A newer server may send operations this web build cannot apply. An
        // expired/replaced cursor also comes here. Retain all data via snapshot.
        sync.borrow_mut().client.require_change_resync();
        fetch_snapshot(&inner, &base, &sync, &busy_done, &selection_key);
    });
    if !live_sync::get_with_status(&url, response) {
        busy.set(false);
    }
    true
}

fn apply_metadata<C: RepaintContext + 'static>(
    inner: &Rc<RefCell<C>>,
    sync: &SharedSync,
    batch: CommittedMetadata,
    expected_pair: (u64, u64),
) -> bool {
    let Ok(mut context) = inner.try_borrow_mut() else {
        return false;
    };
    if current_pair(&*context) != expected_pair || !sync.borrow().gate.pull_allowed(expected_pair) {
        return false;
    }
    let version = batch.version;
    let cursor = batch.cursor.clone();
    let applied = match context
        .host_mut()
        .editor_state_mut()
        .apply_committed_metadata(batch)
    {
        Ok(applied) => applied,
        Err(_) => return false,
    };
    context.host_mut().mark_editor_state_dirty();
    if context.repaint().is_err() {
        if let Some(applied) = applied {
            let _ = context
                .host_mut()
                .editor_state_mut()
                .rollback_committed_metadata(applied);
            context.host_mut().mark_editor_state_dirty();
        }
        return false;
    }
    // Structure/IDs did not change, so neither a full-document serialization
    // nor collab's whole-document identity census is necessary for this pull.
    let pair = current_pair(&*context);
    let mut sync = sync.borrow_mut();
    if !sync.client.commit_change_cursor(version, &cursor) {
        drop(sync);
        if let Some(applied) = applied {
            let _ = context
                .host_mut()
                .editor_state_mut()
                .rollback_committed_metadata(applied);
            context.host_mut().mark_editor_state_dirty();
        }
        return false;
    }
    let state = context.host().editor_state();
    sync.client.note_applied_snapshot_without_hash(
        state.ui.active_page_index,
        state.editor_ui.preserve_authored_geometry,
    );
    sync.gate.note_synced(pair.0, pair.1);
    true
}

/// Original complete-document fallback, shared by first bootstrap, unsupported
/// records and cursor reset. One request at a time and the same apply-time gate.
pub(super) fn fetch_snapshot<C: RepaintContext + 'static>(
    inner: &Rc<RefCell<C>>,
    base: &str,
    sync: &SharedSync,
    busy: &Rc<Cell<bool>>,
    selection_key: &Rc<RefCell<Option<String>>>,
) {
    busy.set(true);
    let inner = Rc::clone(inner);
    let sync = Rc::clone(sync);
    let done = Rc::clone(busy);
    let selection_key = Rc::clone(selection_key);
    let epoch = crate::identity_epoch::epoch();
    let response: Rc<dyn Fn(String)> = Rc::new(move |body| {
        if epoch == crate::identity_epoch::epoch() {
            super::apply_document_response(&inner, &body, &sync, &selection_key);
        }
        done.set(false);
    });
    if !live_sync::get(&format!("{base}/api/mcp/document"), response) {
        busy.set(false);
    }
}
