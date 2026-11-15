//! Borrow only requested first-preorder identities in one page traversal.
use op_editor_ui::layout_scene::{SceneNode, ScenePage};
use std::collections::{HashMap, HashSet};

pub fn resolve_page_nodes<'page, 'id>(
    page: &'page ScenePage,
    node_ids: impl IntoIterator<Item = &'id str>,
) -> HashMap<&'page str, &'page SceneNode> {
    let wanted = node_ids.into_iter().collect::<HashSet<_>>();
    let mut found = HashMap::with_capacity(wanted.len());
    if wanted.is_empty() {
        return found;
    }
    let mut stack = vec![page.children.iter()];
    while let Some(children) = stack.last_mut() {
        let Some(node) = children.next() else {
            stack.pop();
            continue;
        };
        if wanted.contains(node.id.as_str()) {
            found.entry(node.id.as_str()).or_insert(node);
            if found.len() == wanted.len() {
                break;
            }
        }
        if !node.children.is_empty() {
            stack.push(node.children.iter());
        }
    }
    found
}
