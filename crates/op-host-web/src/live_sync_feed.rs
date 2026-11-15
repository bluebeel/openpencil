//! Bounded committed metadata/structural state pulls. Unknown or reset responses
//! explicitly use the complete snapshot path; no command replay.

use super::{current_pair, SharedSync};
use crate::{live_sync, repaint_ctx::RepaintContext};
use op_editor_core::web_sync::CommittedMetadata;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[path = "live_sync_feed_drain.rs"]
mod drain;

pub(super) fn try_fetch<C: RepaintContext + 'static>(
    inner: &Rc<RefCell<C>>,
    base: &str,
    sync: &SharedSync,
    busy: &Rc<Cell<bool>>,
    selection_key: &Rc<RefCell<Option<String>>>,
) -> bool {
    drain::start(inner, base, sync, busy, selection_key)
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
    let structural = batch.changes_structure;
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
    drop(sync);
    // Refresh identities only after accepted structural paint, never after a
    // rejected/rolled-back delta. Metadata avoids this full identity census.
    if structural {
        crate::collab_sync::note_daemon_document(context.host().editor_state());
    }
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
