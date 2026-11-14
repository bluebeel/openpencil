//! One scoped route plan for a move/reorder. Existing records move intact;
//! same-list changes rotate once instead of removing and inserting twice.
//! Actual target identity ambiguity keeps the compatibility resolver.
use crate::{EditorState, NodeId, PenNodeExt};
use jian_ops_schema::node::PenNode;

struct Routes {
    source: Option<Box<[usize]>>,
    target: Option<Box<[usize]>>,
    ambiguous: bool,
}
fn walk_routes(roots: &[PenNode], source: &str, target: Option<&str>) -> Routes {
    let mut found = Routes {
        source: None,
        target: None,
        ambiguous: false,
    };
    let mut stack = vec![(roots, 0)];
    let mut path = Vec::new();
    while let Some((nodes, next)) = stack.last_mut() {
        if *next == nodes.len() {
            stack.pop();
            if !stack.is_empty() {
                path.pop();
            }
            continue;
        }
        let siblings = *nodes;
        let at = *next;
        *next += 1;
        let node = &siblings[at];
        path.push(at);
        let slot = if node.id_str() == source {
            Some(&mut found.source)
        } else if target == Some(node.id_str()) {
            Some(&mut found.target)
        } else {
            None
        };
        if let Some(slot) = slot {
            if slot.is_some() {
                found.ambiguous = true;
            } else {
                *slot = Some(path.clone().into_boxed_slice());
            }
        }
        if let Some(children) = node.children().filter(|children| !children.is_empty()) {
            stack.push((children, 0));
        } else {
            path.pop();
        }
    }
    found
}
fn routes(state: &EditorState, source: &NodeId, target: Option<&NodeId>) -> Routes {
    let scope = state.command_lookup_active_scope();
    if let Some(source_path) = state.command_lookup_unique(&scope, source) {
        let target_path = target.map(|target| state.command_lookup_unique(&scope, target));
        if let Some(None) = target_path {
            return walk_routes(
                state.active_children(),
                source.as_str(),
                target.map(NodeId::as_str),
            );
        }
        let ambiguous = source_path.is_err() || matches!(&target_path, Some(Some(Err(_))));
        return Routes {
            source: source_path.ok().flatten(),
            target: target_path
                .and_then(|path| path.and_then(Result::ok))
                .flatten(),
            ambiguous,
        };
    }
    walk_routes(
        state.active_children(),
        source.as_str(),
        target.map(NodeId::as_str),
    )
}
fn node<'a>(mut nodes: &'a [PenNode], path: &[usize]) -> Option<&'a PenNode> {
    let (last, parents) = path.split_last()?;
    for at in parents {
        nodes = nodes.get(*at)?.children()?;
    }
    nodes.get(*last)
}
fn unlocked(root: &PenNode) -> bool {
    let mut stack = vec![std::slice::from_ref(root).iter()];
    while let Some(siblings) = stack.last_mut() {
        let Some(node) = siblings.next() else {
            stack.pop();
            continue;
        };
        if node.base().locked.unwrap_or(false) {
            return false;
        }
        if let Some(children) = node.children().filter(|children| !children.is_empty()) {
            stack.push(children.iter());
        }
    }
    true
}
fn source_valid(roots: &[PenNode], path: &[usize], require_unlocked: bool) -> bool {
    node(roots, path).is_some_and(|node| !require_unlocked || unlocked(node))
}
fn existing_list<'a>(roots: &'a mut Vec<PenNode>, path: &[usize]) -> &'a mut Vec<PenNode> {
    let mut list = roots;
    for at in path {
        let parent = &mut list[*at];
        debug_assert!(parent.children().is_some());
        list = parent.children_mut().expect("validated existing parent");
    }
    list
}
fn destination_list<'a>(roots: &'a mut Vec<PenNode>, path: &[usize]) -> &'a mut Vec<PenNode> {
    let Some((last, parents)) = path.split_last() else {
        return roots;
    };
    let siblings = existing_list(roots, parents);
    siblings[*last]
        .children_mut()
        .expect("validated insertion container")
}
fn relocate(
    roots: &mut Vec<PenNode>,
    source: &[usize],
    mut parent: Box<[usize]>,
    index: Option<usize>,
) -> bool {
    let (from, source_parent) = source.split_last().unwrap();
    if source_parent == parent.as_ref() {
        let siblings = existing_list(roots, source_parent);
        let to = index.unwrap_or(siblings.len() - 1).min(siblings.len() - 1);
        if *from < to {
            siblings[*from..=to].rotate_left(1);
        } else if *from > to {
            siblings[to..=*from].rotate_right(1);
        }
        return true;
    }
    // Removing an earlier sibling may shift one destination ancestry slot.
    if source.len() <= parent.len()
        && parent[..source_parent.len()] == *source_parent
        && parent[source_parent.len()] > *from
    {
        parent[source_parent.len()] -= 1;
    }
    let moving = existing_list(roots, source_parent).remove(*from);
    let siblings = destination_list(roots, &parent);
    let to = index.unwrap_or(siblings.len()).min(siblings.len());
    siblings.insert(to, moving);
    true
}
pub(crate) fn move_to(
    state: &mut EditorState,
    source: &NodeId,
    parent: &NodeId,
    index: Option<usize>,
    require_unlocked: bool,
) -> Option<bool> {
    if !source.is_real() || source == parent {
        return Some(false);
    }
    let found = routes(state, source, parent.is_real().then_some(parent));
    if found.ambiguous {
        return None;
    }
    let Some(source) = found.source else {
        return Some(false);
    };
    if !source_valid(state.active_children(), &source, require_unlocked) {
        return Some(false);
    }
    let destination = if parent.is_real() {
        let Some(target) = found.target else {
            return Some(false);
        };
        if target.starts_with(&source)
            || !node(state.active_children(), &target).is_some_and(PenNodeExt::is_container)
        {
            return Some(false);
        }
        target
    } else {
        Box::new([])
    };
    Some(relocate(
        state.active_children_mut(),
        &source,
        destination,
        index,
    ))
}
pub(crate) fn relative(
    state: &mut EditorState,
    source: &NodeId,
    anchor: &NodeId,
    before: bool,
) -> Option<bool> {
    if !source.is_real() || !anchor.is_real() || source == anchor {
        return Some(false);
    }
    let found = routes(state, source, Some(anchor));
    if found.ambiguous {
        return None;
    }
    let (Some(source), Some(target)) = (found.source, found.target) else {
        return Some(false);
    };
    if target.starts_with(&source) || !source_valid(state.active_children(), &source, true) {
        return Some(false);
    }
    let (anchor, parent) = target.split_last().unwrap();
    let same_parent = source[..source.len() - 1] == *parent;
    let adjusted = *anchor - usize::from(same_parent && source[source.len() - 1] < *anchor);
    let index = adjusted + usize::from(!before);
    Some(relocate(
        state.active_children_mut(),
        &source,
        parent.into(),
        Some(index),
    ))
}
