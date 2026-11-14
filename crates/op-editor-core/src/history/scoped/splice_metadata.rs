//! Prepare metadata destinations alongside structural splices before mutation.
//! Only unchanged existing identities survive; removed or absence-changing
//! destinations refuse atomically instead of becoming a partial undo.
use super::*;
pub(super) fn relocations(
    nodes: &[PenNode],
    removed: &HashSet<String>,
    order: &[Token],
) -> Vec<Option<usize>> {
    let count = nodes
        .iter()
        .filter(|node| !removed.contains(node.id_str()))
        .count();
    let mut positions = vec![None; count];
    for (at, token) in order.iter().enumerate() {
        if let Token::Existing(index) = token {
            positions[*index] = Some(at);
        }
    }
    let mut next = 0usize;
    nodes
        .iter()
        .map(|node| {
            if removed.contains(node.id_str()) {
                None
            } else {
                let at = positions[next];
                next += 1;
                at
            }
        })
        .collect()
}
pub(super) fn routes(
    doc: &PenDocument,
    patches: &[Patch],
    index: &identity::Index,
    parents: &[ParentPlan],
) -> Result<Vec<(usize, Box<[usize]>)>, ScopedHistoryError> {
    let mut out = Vec::new();
    for (at, patch) in patches.iter().enumerate() {
        let Patch::Node(node) = patch else {
            continue;
        };
        if !node.metadata_only {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        let mut path = index.path(&node.scope, &node.expected_id)?;
        // Source validation still uses the pre-exchange location. The planned
        // route below becomes active only after all splices have exchanged.
        let roots = patch::roots(doc, &node.scope)?;
        if patch::node(roots, &path).is_none() {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        for parent in parents {
            if parent.scope != node.scope || !path.starts_with(&parent.path) {
                continue;
            }
            let depth = parent.path.len();
            if path.len() == depth {
                if parent.absence {
                    return Err(ScopedHistoryError::NodePathChanged);
                }
            } else {
                path[depth] = parent
                    .relocation
                    .get(path[depth])
                    .copied()
                    .flatten()
                    .ok_or(ScopedHistoryError::NodePathChanged)?;
            }
        }
        out.push((at, path));
    }
    Ok(out)
}
