//! Borrowed catalog proof for exactly the clone resolver's canonical sources.
//! Fresh refs to existing reusable masters do not register new definitions;
//! copied nested reusable definitions retain the complete catalog checkpoint.
use super::*;
use crate::EditorCommand as C;
fn no_definitions(node: &PenNode) -> bool {
    let mut pending = vec![node];
    while let Some(node) = pending.pop() {
        if reusable(node) {
            return false;
        }
        if let Some(children) = node.children() {
            pending.extend(children);
        }
    }
    true
}
pub(crate) fn preserves(state: &EditorState, command: &C) -> bool {
    let roots = state.active_children();
    match command {
        C::DuplicateSelected { .. } => {
            let wanted = state
                .selection
                .set
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect();
            let index = lookup::Index::new(roots, wanted);
            for id in &state.selection.set {
                if index.ambiguous(id.as_str()) {
                    return false;
                }
                if let Some(source) = index.original.get(id.as_str()) {
                    let Some(node) = lookup::at(roots, &source.sibling_first) else {
                        return false;
                    };
                    if !reusable(node) && !no_definitions(node) {
                        return false;
                    }
                }
                // A formerly missing selected ID can name an earlier output.
                // Every such output derives from these same proved sources.
            }
            true
        }
        C::PasteClipboard { .. } => {
            let wanted = state
                .clipboard
                .iter()
                .filter(|node| reusable(node))
                .map(|node| node.id_str().to_owned())
                .collect();
            let index = lookup::Index::new(roots, wanted);
            for node in &state.clipboard {
                let existing = reusable(node)
                    && !index.ambiguous(node.id_str())
                    && index
                        .original
                        .get(node.id_str())
                        .and_then(|source| lookup::at(roots, &source.preorder))
                        .is_some_and(reusable);
                if !existing && !no_definitions(node) {
                    return false;
                }
            }
            true
        }
        _ => false,
    }
}
