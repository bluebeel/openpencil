//! Exact target checkpoints for a host-proven no-document-repair command.
use super::{patch, ScopedHistoryError};
use crate::{EditorState, PenNodeExt, SharedNode};
use jian_ops_schema::node::PenNode;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TargetSpec {
    pub scope: patch::Scope,
    pub path: Box<[usize]>,
    pub subtree: bool,
    pub splice: Option<super::SpliceSpec>,
    pub topology: Option<super::TopologySpec>,
}
#[derive(Debug)]
pub(super) struct TargetCheckpoint {
    spec: TargetSpec,
    old: Option<Arc<SharedNode>>,
    splice: Option<super::splice::PendingSplice>,
    topology: Option<super::topology::PendingTopology>,
}
pub(super) fn capture(
    state: &EditorState,
    specs: Vec<TargetSpec>,
) -> Result<Vec<TargetCheckpoint>, ScopedHistoryError> {
    if specs.len() > 4096 || specs.len() != 1 && specs.iter().any(|spec| spec.topology.is_some()) {
        return Err(ScopedHistoryError::NodePathChanged);
    }
    if specs.iter().any(|spec| spec.splice.is_some()) && specs.iter().any(|spec| spec.subtree) {
        return Err(ScopedHistoryError::NodePathChanged);
    }
    if specs.len() > 1 && specs.iter().any(|spec| spec.splice.is_some()) {
        let mut wanted = super::identity::Wanted::new();
        let mut expected = Vec::new();
        for spec in &specs {
            if spec.path.is_empty() {
                continue;
            }
            let node = patch::node(patch::roots(&state.doc, &spec.scope)?, &spec.path)
                .ok_or(ScopedHistoryError::NodePathChanged)?;
            wanted
                .entry(spec.scope.clone())
                .or_default()
                .insert(node.id_str().to_owned());
            expected.push((
                spec.scope.clone(),
                node.id_str().to_owned(),
                spec.path.clone(),
            ));
        }
        let identities = super::identity::Index::new(&state.doc, wanted)?;
        for (scope, id, path) in expected {
            if identities.path(&scope, &id)? != path {
                return Err(ScopedHistoryError::NodePathChanged);
            }
        }
    }
    specs
        .into_iter()
        .map(|spec| {
            if let Some(edit) = &spec.topology {
                let topology = super::topology::PendingTopology::capture(
                    state,
                    spec.scope.clone(),
                    spec.path.clone(),
                    edit,
                )?;
                return Ok(TargetCheckpoint {
                    spec,
                    old: None,
                    splice: None,
                    topology: Some(topology),
                });
            }
            if let Some(edit) = &spec.splice {
                let splice = super::splice::PendingSplice::capture(
                    state,
                    spec.scope.clone(),
                    spec.path.clone(),
                    edit,
                )?;
                return Ok(TargetCheckpoint {
                    spec,
                    old: None,
                    splice: Some(splice),
                    topology: None,
                });
            }
            let node = patch::node(patch::roots(&state.doc, &spec.scope)?, &spec.path)
                .ok_or(ScopedHistoryError::NodePathChanged)?;
            let old = if spec.subtree {
                SharedNode::capture(node, None)
            } else {
                SharedNode::capture_header(node)
            };
            Ok(TargetCheckpoint {
                spec,
                old: Some(old),
                splice: None,
                topology: None,
            })
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
        checkpoints
            .iter()
            .map(|target| {
                target.old.as_ref().map_or_else(
                    || target.splice.as_ref().map_or(0, |splice| splice.counts().1),
                    |old| children(old),
                )
            })
            .sum(),
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
        if let Some(topology) = &checkpoint.topology {
            return topology.matches(state);
        }
        if let Some(splice) = &checkpoint.splice {
            return splice.matches(state);
        }
        let old = checkpoint.old.as_ref().unwrap();
        let Ok(roots) = patch::roots(&state.doc, &checkpoint.spec.scope) else {
            return false;
        };
        let Some(current) = patch::node(roots, &checkpoint.spec.path) else {
            return false;
        };
        if checkpoint.spec.subtree {
            old.equals_node(current)
        } else {
            old.equals_header(current)
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
        if checkpoint.splice.is_some() || checkpoint.topology.is_some() {
            continue;
        }
        let old = checkpoint.old.as_ref().unwrap();
        let current = patch::node(
            patch::roots(&state.doc, &checkpoint.spec.scope)?,
            &checkpoint.spec.path,
        )
        .ok_or(ScopedHistoryError::NodePathChanged)?;
        if !checkpoint.spec.subtree
            && (std::mem::discriminant(current) != std::mem::discriminant(old.metadata())
                || current.children().is_some() != old.children().is_some())
        {
            return Err(ScopedHistoryError::NodePathChanged);
        }
    }
    for checkpoint in checkpoints {
        if let Some(topology) = checkpoint.topology {
            topology.restore(state)?;
            continue;
        }
        if let Some(splice) = checkpoint.splice {
            splice.restore(state)?;
            continue;
        }
        let old = checkpoint.old.unwrap();
        let current = patch::node_mut(
            patch::roots_mut(&mut state.doc, &checkpoint.spec.scope)?,
            &checkpoint.spec.path,
        )
        .unwrap();
        if checkpoint.spec.subtree {
            *current = old.materialize();
        } else {
            replace_header(current, old.materialize())?;
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
        if let Some(topology) = checkpoint.topology {
            if let Some(patch) = topology.compact(state)? {
                out.push(patch::Patch::Topology(patch));
            }
            continue;
        }
        if let Some(splice) = checkpoint.splice {
            if let Some(patch) = splice.compact(state)? {
                out.push(patch::Patch::Splice(patch));
            }
            continue;
        }
        let old = checkpoint.old.unwrap();
        let current = patch::node(
            patch::roots(&state.doc, &checkpoint.spec.scope)?,
            &checkpoint.spec.path,
        )
        .ok_or(ScopedHistoryError::NodePathChanged)?;
        if !checkpoint.spec.subtree
            && (std::mem::discriminant(old.metadata()) != std::mem::discriminant(current)
                || old.children().is_some() != current.children().is_some())
        {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        let same = if checkpoint.spec.subtree {
            old.equals_node(current)
        } else {
            old.equals_header(current)
        };
        if !same {
            out.push(patch::Patch::Node(patch::NodePatch {
                scope: checkpoint.spec.scope,
                path: checkpoint.spec.path,
                expected_id: current.id_str().to_owned(),
                metadata_only: !checkpoint.spec.subtree,
                affected_ids: checkpoint
                    .spec
                    .subtree
                    .then(|| super::identity::changed_subtree_ids(&old, current)),
                old,
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

/// Structural requests capture one command and cannot be mixed with metadata
/// targets. All paths are checked again before any trusted direct mutation.
pub(super) fn matches_topology_request(
    state: &EditorState,
    command: &crate::EditorCommand,
    checkpoints: &[TargetCheckpoint],
) -> Option<bool> {
    let [checkpoint] = checkpoints else {
        return None;
    };
    checkpoint
        .spec
        .topology
        .as_ref()
        .map(|spec| spec.request.matches(state, command))
}
pub(super) fn apply_topology(
    state: &mut EditorState,
    checkpoints: &mut [TargetCheckpoint],
) -> Option<bool> {
    let [checkpoint] = checkpoints else {
        return None;
    };
    checkpoint
        .topology
        .as_mut()
        .map(|pending| pending.apply_direct(state))
}

/// Only the atomic metadata prepare/apply entry point calls this. No mutable
/// document borrow escapes between first-ID resolution and this direct write.
pub(super) fn apply_metadata_direct(
    state: &mut EditorState,
    command: &crate::EditorCommand,
    prepared: &crate::command_node_attrs::bulk::Prepared,
    checkpoints: &mut [TargetCheckpoint],
) -> Option<bool> {
    let [checkpoint] = checkpoints else {
        return None;
    };
    if checkpoint.spec.subtree
        || checkpoint.spec.splice.is_some()
        || checkpoint.spec.topology.is_some()
    {
        return None;
    }
    let node = patch::node_mut(
        patch::roots_mut(&mut state.doc, &checkpoint.spec.scope).ok()?,
        &checkpoint.spec.path,
    )?;
    let applied = crate::command_node_attrs::bulk::write(node, command, prepared);
    if applied && crate::command_apply::command_marks_document_dirty(command) {
        state.mark_document_changed();
    }
    Some(applied)
}
