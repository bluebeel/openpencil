//! Moved before-state regions for an exclusively borrowed feed transaction.
//! Paths outside a replaced child/subtree region cannot move. Covered regions
//! are absorbed into an outer capture, keeping all surviving captures disjoint.

use super::patch::{self, Scope};
use crate::{PenDocument, PenNodeExt};
use jian_ops_schema::node::PenNode;

pub(super) enum Data {
    Header(PenNode),
    Node(PenNode),
    Children(Option<Vec<PenNode>>),
}
pub(super) struct Before {
    pub scope: Scope,
    pub path: Vec<usize>,
    pub data: Data,
}
#[derive(Clone, Copy)]
pub(super) enum Kind {
    Header,
    Node,
    Children,
}
impl Before {
    pub(super) fn covers(&self, scope: &Scope, path: &[usize], kind: Kind) -> bool {
        if &self.scope != scope {
            return false;
        }
        match self.data {
            Data::Header(_) => matches!(kind, Kind::Header) && self.path == path,
            Data::Node(_) => path.starts_with(&self.path),
            Data::Children(_) => {
                path.starts_with(&self.path)
                    && (path.len() > self.path.len() || matches!(kind, Kind::Children))
            }
        }
    }
    pub(super) fn kind(&self) -> Kind {
        match self.data {
            Data::Header(_) => Kind::Header,
            Data::Node(_) => Kind::Node,
            Data::Children(_) => Kind::Children,
        }
    }
    /// Restore a contained capture into this moved outer BEFORE value. This
    /// reconstructs the original baseline even when earlier feed records had
    /// already edited the descendant before its parent was replaced.
    pub(super) fn absorb(&mut self, inner: Before) {
        let relative = inner.path.strip_prefix(&self.path).expect("covered path");
        match &mut self.data {
            Data::Node(root) => {
                let node = if relative.is_empty() {
                    root
                } else {
                    patch::node_mut(
                        patch::children_slot(root)
                            .and_then(Option::as_mut)
                            .expect("captured subtree path"),
                        relative,
                    )
                    .expect("captured descendant")
                };
                inner.data.restore_node(node);
            }
            Data::Children(roots) => {
                if relative.is_empty() {
                    let Data::Children(before) = inner.data else {
                        unreachable!("child forest does not cover parent header")
                    };
                    *roots = before;
                } else {
                    let node =
                        patch::node_mut(roots.as_mut().expect("captured child forest"), relative)
                            .expect("captured child path");
                    inner.data.restore_node(node);
                }
            }
            Data::Header(_) => unreachable!("duplicate headers reuse the first capture"),
        }
    }
    pub(super) fn restore(self, doc: &mut PenDocument) {
        let roots = patch::roots_mut(doc, &self.scope).expect("transaction page shape fixed");
        if self.path.is_empty() {
            let Data::Children(Some(before)) = self.data else {
                unreachable!("document roots stay present")
            };
            *roots = before;
        } else {
            self.data
                .restore_node(patch::node_mut(roots, &self.path).expect("disjoint rollback path"));
        }
    }
}
impl Data {
    fn restore_node(self, node: &mut PenNode) {
        match self {
            Self::Node(before) => *node = before,
            Self::Header(mut before) => {
                let children = patch::children_slot(node).and_then(Option::take);
                if let Some(slot) = patch::children_slot(&mut before) {
                    *slot = children;
                }
                *node = before;
            }
            Self::Children(before) => {
                *patch::children_slot(node).expect("captured container") = before
            }
        }
    }
}

pub(super) fn remember(before: &mut Vec<Before>, mut captured: Before) {
    if before
        .iter()
        .any(|old| old.covers(&captured.scope, &captured.path, captured.kind()))
    {
        return;
    }
    let mut retained = Vec::with_capacity(before.len() + 1);
    for old in std::mem::take(before) {
        if captured.covers(&old.scope, &old.path, old.kind()) {
            captured.absorb(old);
        } else {
            retained.push(old);
        }
    }
    retained.push(captured);
    *before = retained;
}

/// Move a node header while preserving every real child allocation/presence.
pub(super) fn replace_header(node: &mut PenNode, mut value: PenNode) -> PenNode {
    let children = patch::children_slot(node).and_then(Option::take);
    let present = children.is_some();
    if let Some(slot) = patch::children_slot(&mut value) {
        *slot = children;
    }
    let mut old = std::mem::replace(node, value);
    if let Some(slot) = patch::children_slot(&mut old) {
        *slot = present.then(Vec::new);
    }
    old
}

/// Child absence changes need a full parent swap in existing history. Merge
/// its independently captured header, retaining all original metadata exactly.
pub(super) fn normalize(before: &mut Vec<Before>, doc: &PenDocument) {
    let mut at = 0;
    while at < before.len() {
        let needs_parent = match &before[at].data {
            Data::Children(old) if !before[at].path.is_empty() => {
                let node = patch::node(
                    patch::roots(doc, &before[at].scope).unwrap(),
                    &before[at].path,
                )
                .unwrap();
                old.is_some() != node.children().is_some()
                    || before.iter().any(|header| header.scope == before[at].scope && header.path == before[at].path
                        && matches!(&header.data, Data::Header(old) if old.id_str() != node.id_str()))
            }
            _ => false,
        };
        if !needs_parent {
            at += 1;
            continue;
        }
        let region = before.remove(at);
        let Data::Children(children) = region.data else {
            unreachable!()
        };
        let header = before.iter().position(|old| {
            old.scope == region.scope
                && old.path == region.path
                && matches!(old.data, Data::Header(_))
        });
        let mut node = if let Some(header) = header {
            let Data::Header(header) = before.remove(header).data else {
                unreachable!()
            };
            header
        } else {
            crate::pen_node_ext::clone_without_children(
                patch::node(patch::roots(doc, &region.scope).unwrap(), &region.path).unwrap(),
            )
        };
        *patch::children_slot(&mut node).unwrap() = children;
        before.push(Before {
            scope: region.scope,
            path: region.path,
            data: Data::Node(node),
        });
        // Removal can move a previous slot; restart the small capture list.
        at = 0;
    }
}
