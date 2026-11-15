//! Immutable sibling-first planning keeps a large bounced node payload out of
//! recursive search frames. No route escapes the lexical exclusive caller.
use super::{stack, NodeId, PenNode, PenNodeExt};

pub(super) enum Route {
    Root(usize),
    Nested(Vec<usize>),
}
impl std::ops::Deref for Route {
    type Target = [usize];
    fn deref(&self) -> &[usize] {
        match self {
            Self::Root(at) => std::slice::from_ref(at),
            Self::Nested(path) => path,
        }
    }
}

/// The recursive compatibility walkers test direct siblings before descending.
/// A direct match whose action is impossible returns from exactly that scope:
/// its caller can then continue the next ancestor sibling. Do not search a
/// rejected scope's descendants or retry a later same-level duplicate.
pub(super) fn find(
    roots: &[PenNode],
    target: &NodeId,
    accept: impl Fn(&[PenNode], usize) -> bool,
) -> Option<Route> {
    struct Frame<'a> {
        siblings: &'a [PenNode],
        next: usize,
        entered: bool,
    }
    if roots.is_empty() {
        return None;
    }
    if let Some(at) = roots
        .iter()
        .position(|node| node.id_str() == target.as_str())
    {
        return accept(roots, at).then_some(Route::Root(at));
    }
    let mut pending = stack::Stack::<_, 16>::new();
    let mut path = Vec::new();
    pending.push(Frame {
        siblings: roots,
        next: 0,
        entered: true,
    });
    while let Some(frame) = pending.last_mut() {
        if !frame.entered {
            frame.entered = true;
            if let Some(at) = frame
                .siblings
                .iter()
                .position(|node| node.id_str() == target.as_str())
            {
                if accept(frame.siblings, at) {
                    path.push(at);
                    return Some(Route::Nested(path));
                }
                pending.pop();
                path.pop();
                continue;
            }
        }
        let at = frame.next;
        let siblings = frame.siblings;
        let Some(node) = siblings.get(at) else {
            pending.pop();
            path.pop();
            continue;
        };
        frame.next += 1;
        if let Some(children) = node.children().filter(|children| !children.is_empty()) {
            path.push(at);
            pending.push(Frame {
                siblings: children,
                next: 0,
                entered: false,
            });
        }
    }
    None
}

pub(super) fn vec_mut<'a>(
    roots: &'a mut Vec<PenNode>,
    parents: &[usize],
) -> Option<&'a mut Vec<PenNode>> {
    let mut siblings = roots;
    for at in parents {
        siblings = siblings.get_mut(*at)?.children_mut_if_present()?;
    }
    Some(siblings)
}

pub(super) fn slice_mut<'a>(
    roots: &'a mut [PenNode],
    parents: &[usize],
) -> Option<&'a mut [PenNode]> {
    let mut siblings = roots;
    for at in parents {
        siblings = siblings.get_mut(*at)?.children_mut_if_present()?;
    }
    Some(siblings)
}

pub(super) fn node_mut<'a>(roots: &'a mut [PenNode], path: &[usize]) -> Option<&'a mut PenNode> {
    let (at, parents) = path.split_last()?;
    slice_mut(roots, parents)?.get_mut(*at)
}
