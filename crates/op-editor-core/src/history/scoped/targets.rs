//! Exact target checkpoints for a host-proven no-document-repair command.
use super::{patch, ScopedHistoryError};
use crate::{pen_node_ext::clone_without_children, EditorState, PenNodeExt, SharedNode};
use jian_ops_schema::node::PenNode;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TargetSpec {
    pub scope: patch::Scope,
    pub path: Box<[usize]>,
    pub subtree: bool,
}
#[derive(Debug)]
pub(super) struct TargetCheckpoint {
    spec: TargetSpec,
    old: Arc<SharedNode>,
}
pub(super) fn capture(
    state: &EditorState,
    specs: Vec<TargetSpec>,
) -> Result<Vec<TargetCheckpoint>, ScopedHistoryError> {
    specs
        .into_iter()
        .map(|spec| {
            let node = patch::node(patch::roots(&state.doc, &spec.scope)?, &spec.path)
                .ok_or(ScopedHistoryError::NodePathChanged)?;
            let old = if spec.subtree {
                SharedNode::capture(node, None)
            } else {
                SharedNode::capture(&clone_without_children(node), None)
            };
            Ok(TargetCheckpoint { spec, old })
        })
        .collect()
}
#[cfg(test)]
pub(super) fn checkpoint_counts(checkpoints: &[TargetCheckpoint]) -> (usize, usize) {
    fn children(node: &SharedNode) -> usize {
        node.children()
            .map_or(0, |nodes| nodes.iter().map(|node| 1 + children(node)).sum())
    }
    (
        checkpoints.len(),
        checkpoints.iter().map(|target| children(&target.old)).sum(),
    )
}
pub(super) fn matches_specs(checkpoints: &[TargetCheckpoint], specs: &[TargetSpec]) -> bool {
    checkpoints.len() == specs.len()
        && checkpoints
            .iter()
            .zip(specs)
            .all(|(old, spec)| old.spec == *spec)
}
pub(super) fn matches(state: &EditorState, checkpoints: &[TargetCheckpoint]) -> bool {
    checkpoints.iter().all(|checkpoint| {
        let Ok(roots) = patch::roots(&state.doc, &checkpoint.spec.scope) else {
            return false;
        };
        let Some(current) = patch::node(roots, &checkpoint.spec.path) else {
            return false;
        };
        if checkpoint.spec.subtree {
            checkpoint.old.equals_node(current)
        } else {
            checkpoint.old.equals_node(&clone_without_children(current))
        }
    })
}
pub(super) fn restore(
    state: &mut EditorState,
    checkpoints: Vec<TargetCheckpoint>,
) -> Result<(), ScopedHistoryError> {
    // All paths are checked before the first write; target IDs may intentionally
    // differ after a whole-node replacement at this same structural slot.
    for checkpoint in &checkpoints {
        let current = patch::node(
            patch::roots(&state.doc, &checkpoint.spec.scope)?,
            &checkpoint.spec.path,
        )
        .ok_or(ScopedHistoryError::NodePathChanged)?;
        if !checkpoint.spec.subtree
            && (std::mem::discriminant(current)
                != std::mem::discriminant(checkpoint.old.metadata())
                || current.children().is_some() != checkpoint.old.children().is_some())
        {
            return Err(ScopedHistoryError::NodePathChanged);
        }
    }
    for checkpoint in checkpoints {
        let current = patch::node_mut(
            patch::roots_mut(&mut state.doc, &checkpoint.spec.scope)?,
            &checkpoint.spec.path,
        )
        .unwrap();
        if checkpoint.spec.subtree {
            *current = checkpoint.old.materialize();
        } else {
            replace_header(current, checkpoint.old.materialize())?;
        }
    }
    Ok(())
}
pub(super) fn compact(
    state: &EditorState,
    checkpoints: Vec<TargetCheckpoint>,
) -> Result<Vec<patch::Patch>, ScopedHistoryError> {
    let mut out = Vec::with_capacity(checkpoints.len());
    for checkpoint in checkpoints {
        let current = patch::node(
            patch::roots(&state.doc, &checkpoint.spec.scope)?,
            &checkpoint.spec.path,
        )
        .ok_or(ScopedHistoryError::NodePathChanged)?;
        if !checkpoint.spec.subtree
            && (std::mem::discriminant(checkpoint.old.metadata())
                != std::mem::discriminant(current)
                || checkpoint.old.children().is_some() != current.children().is_some())
        {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        let same = if checkpoint.spec.subtree {
            checkpoint.old.equals_node(current)
        } else {
            checkpoint.old.equals_node(&clone_without_children(current))
        };
        if !same {
            out.push(patch::Patch::Node(patch::NodePatch {
                scope: checkpoint.spec.scope,
                path: checkpoint.spec.path,
                expected_id: current.id_str().to_owned(),
                metadata_only: !checkpoint.spec.subtree,
                old: checkpoint.old,
            }));
        }
    }
    Ok(out)
}
fn replace_header(current: &mut PenNode, mut header: PenNode) -> Result<(), ScopedHistoryError> {
    if std::mem::discriminant(current) != std::mem::discriminant(&header)
        || current.children().is_some() != header.children().is_some()
    {
        return Err(ScopedHistoryError::NodePathChanged);
    }
    let children = patch::children_slot(current).and_then(Option::take);
    if let Some(slot) = patch::children_slot(&mut header) {
        *slot = children;
    }
    *current = header;
    Ok(())
}
