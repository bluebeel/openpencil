//! Legacy global read identity policies over an explicit locked-host index.
use jian_ops_schema::node::PenNode;
use op_editor_core::{EditorState, NodeRead, NodeReadOrder, PenNodeExt};
fn reverse_forests(state: &EditorState) -> impl Iterator<Item = &[PenNode]> {
    state
        .doc
        .pages
        .iter()
        .flat_map(|pages| pages.iter().rev().map(|page| page.children.as_slice()))
        .chain(
            state
                .doc
                .pages
                .is_none()
                .then_some(state.doc.children.as_slice()),
        )
}
pub(super) fn global_last<'a>(state: &'a EditorState, id: &str) -> Option<Option<NodeRead<'a>>> {
    for roots in reverse_forests(state) {
        let lookup = state
            .trusted_node_lookup(roots)
            .filter(|lookup| lookup.is_indexed())?;
        if let Some(found) = lookup.find(id, NodeReadOrder::Last) {
            return Some(Some(found));
        }
    }
    Some(None)
}
pub(super) fn global_children<'a>(
    state: &'a EditorState,
    id: &str,
) -> Option<(bool, Option<&'a [PenNode]>)> {
    let mut known = false;
    for roots in reverse_forests(state) {
        let lookup = state
            .trusted_node_lookup(roots)
            .filter(|lookup| lookup.is_indexed())?;
        if let Some(found) = lookup.find(id, NodeReadOrder::LastNonEmptyChildren) {
            return Some((true, found.node.children().map(Vec::as_slice)));
        }
        known |= lookup.find(id, NodeReadOrder::First).is_some();
    }
    Some((known, None))
}
