//! Scoped inline-text history and canonical String reuse.
use super::{text_content_node_mut, EditorState, NodeId, TEXT_COALESCE_MS};
use crate::PenNodeExt;
use jian_ops_schema::node::PenNode;

impl EditorState {
    pub(super) fn text_edit_sync_input_to_node(
        &mut self,
        id: &NodeId,
        now_ms: u64,
        revision_before: u64,
    ) -> bool {
        let last = self.ui.text_edit_last_ms;
        if last == 0 || now_ms.saturating_sub(last) > TEXT_COALESCE_MS {
            // Only the first key of a burst needs an owned before-header.
            // Canonical text uses one scoped undo entry; materialized instance
            // editing retains its existing complete-history fallback.
            let draft = self.ui.text_edit_input.text().to_owned();
            if let Some(wrote) = self.try_edit_header_with_history(id, |node| {
                let Some(content) = text_content_node_mut(node) else {
                    return false;
                };
                if *content != draft {
                    content.clear();
                    content.push_str(&draft);
                }
                true
            }) {
                self.ui.pending_text_edit_history = None;
                return wrote;
            }
            self.maybe_open_text_edit_history(now_ms);
        }
        let scope = self.command_lookup_active_scope();
        let route = self.command_lookup_path(
            &scope,
            id,
            crate::command_id_scope::sequence::LookupOrder::Preorder,
        );
        let text = self.ui.text_edit_input.text();
        let roots = match self.doc.pages.as_mut().filter(|pages| !pages.is_empty()) {
            Some(pages) => {
                let index = self.ui.active_page_index.min(pages.len() - 1);
                &mut pages[index].children
            }
            None => &mut self.doc.children,
        };
        let node = match route {
            Some(Some(path)) => {
                node_at_path_mut(roots, &path).filter(|node| node.id_str() == id.as_str())
            }
            Some(None) => None,
            None => crate::walkers::find_node_mut(roots, id),
        };
        let Some(content) = node.and_then(text_content_node_mut) else {
            return false;
        };
        if content != text {
            // Reuse the canonical String allocation, avoiding a second draft
            // copy. Every key invalidates snapshots, including coalesced keys
            // after an intervening save/render.
            content.clear();
            content.push_str(text);
            if self.document_revision() == revision_before {
                self.mark_document_changed();
            }
        }
        true
    }

    /// Open a fresh history burst when the first keystroke since the
    /// session started, or > 500 ms since the last one. Snapshots the
    /// pre-mutation state and pushes it onto the undo stack.
    fn maybe_open_text_edit_history(&mut self, now_ms: u64) {
        let last = self.ui.text_edit_last_ms;
        let elapsed = now_ms.saturating_sub(last);
        if last == 0 || elapsed > TEXT_COALESCE_MS {
            let snap = self.snapshot_for_history();
            self.ui.pending_text_edit_history = None;
            self.history_push_past(snap);
        }
    }
}

fn node_at_path_mut<'a>(roots: &'a mut [PenNode], path: &[usize]) -> Option<&'a mut PenNode> {
    let (first, tail) = path.split_first()?;
    let mut node = roots.get_mut(*first)?;
    for index in tail {
        node = node.children_mut_if_present()?.get_mut(*index)?;
    }
    Some(node)
}
