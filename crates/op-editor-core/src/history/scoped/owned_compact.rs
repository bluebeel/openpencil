//! Compact moved before-state against borrowed final state. Unlike broad legacy
//! captures, removed/replaced subtrees transfer into history without cloning.
use super::*;

pub(crate) fn node(
    mut old: PenNode,
    current: &PenNode,
    scope: &Scope,
    path: &mut Vec<usize>,
    out: &mut Vec<Patch>,
) {
    if old.id_str() != current.id_str()
        || mem::discriminant(&old) != mem::discriminant(current)
        || old.children().is_some() != current.children().is_some()
    {
        let old = SharedNode::capture_owned(old);
        let affected_ids = Some(super::super::identity::changed_subtree_ids(&old, current));
        out.push(Patch::Node(NodePatch {
            scope: scope.clone(),
            path: path.clone().into(),
            expected_id: current.id_str().to_owned(),
            metadata_only: false,
            affected_ids,
            old,
        }));
        return;
    }
    let children = children_slot(&mut old).and_then(Option::take);
    if children.is_some() {
        *children_slot(&mut old).unwrap() = Some(Vec::new());
    }
    header(old, current, scope, path, out);
    if let Some(children) = children {
        list(
            children,
            current.children().unwrap(),
            scope,
            path,
            Some(current.id_str()),
            out,
        );
    }
}
pub(crate) fn header(
    old: PenNode,
    current: &PenNode,
    scope: &Scope,
    path: &[usize],
    out: &mut Vec<Patch>,
) {
    let metadata = clone_without_children(current);
    if !crate::history_snapshot::metadata_matches(&old, &metadata) {
        out.push(Patch::Node(NodePatch {
            scope: scope.clone(),
            path: path.into(),
            expected_id: current.id_str().to_owned(),
            metadata_only: true,
            affected_ids: None,
            old: SharedNode::capture_owned(old),
        }));
    }
}
pub(crate) fn list(
    old: Vec<PenNode>,
    current: &[PenNode],
    scope: &Scope,
    parent: &mut Vec<usize>,
    parent_id: Option<&str>,
    out: &mut Vec<Patch>,
) {
    if old.len() != current.len()
        || old
            .iter()
            .zip(current)
            .any(|(a, b)| a.id_str() != b.id_str())
    {
        let by_id: HashMap<_, _> = current
            .iter()
            .enumerate()
            .map(|(at, node)| (node.id_str(), (at, node)))
            .collect();
        let unique = old
            .iter()
            .map(PenNodeExt::id_str)
            .collect::<std::collections::HashSet<_>>()
            .len();
        if by_id.len() == current.len() && unique == old.len() {
            let old_order = old.iter().map(|node| node.id_str().to_owned()).collect();
            let expected_ids = current
                .iter()
                .map(|node| node.id_str().to_owned())
                .collect();
            let mut removed = Vec::new();
            for before in old {
                if let Some((at, after)) = by_id.get(before.id_str()) {
                    parent.push(*at);
                    node(before, after, scope, parent, out);
                    parent.pop();
                } else {
                    removed.push(SharedNode::capture_owned(before));
                }
            }
            out.push(Patch::Order(OrderPatch {
                scope: scope.clone(),
                parent: parent.clone().into(),
                parent_id: parent_id.map(str::to_owned),
                old_order,
                expected_ids,
                old: removed,
            }));
            return;
        }
        out.push(Patch::Forest(ForestPatch {
            scope: scope.clone(),
            parent: parent.clone().into(),
            parent_id: parent_id.map(str::to_owned),
            expected_ids: current
                .iter()
                .map(|node| node.id_str().to_owned())
                .collect(),
            old: old.into_iter().map(SharedNode::capture_owned).collect(),
        }));
        return;
    }
    for (at, (before, after)) in old.into_iter().zip(current).enumerate() {
        parent.push(at);
        node(before, after, scope, parent, out);
        parent.pop();
    }
}
