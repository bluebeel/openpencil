//! Resolve current destinations by identity before applying selective history.
use super::{
    identity,
    patch::{self, Patch},
    ScopedHistoryError,
};
use crate::{PenDocument, PenNodeExt};
use std::collections::HashSet;
pub(super) fn paths(doc: &PenDocument, patches: &mut [Patch]) -> Result<(), ScopedHistoryError> {
    let mut wanted = identity::Wanted::new();
    for patch in &*patches {
        match patch {
            Patch::Node(node) => {
                wanted
                    .entry(node.scope.clone())
                    .or_default()
                    .insert(node.expected_id.clone());
            }
            Patch::Topology(topology) => topology.want_identities(&mut wanted),
            _ => {
                for splice in patch.splices() {
                    if let Some(id) = &splice.parent_id {
                        wanted
                            .entry(splice.scope.clone())
                            .or_default()
                            .insert(id.clone());
                    }
                }
            }
        }
    }
    let index = identity::Index::new(doc, wanted)?;
    for patch in patches {
        match patch {
            Patch::Node(node) => node.path = index.path(&node.scope, &node.expected_id)?,
            Patch::Topology(topology) => topology.rebase_identity(&index)?,
            _ => {
                for splice in patch.splices_mut() {
                    if let Some(id) = &splice.parent_id {
                        splice.parent = index.path(&splice.scope, id)?;
                    }
                }
            }
        }
    }
    Ok(())
}
pub(super) fn restored_node_collisions(
    doc: &PenDocument,
    patches: &[Patch],
) -> Result<(), ScopedHistoryError> {
    let mut introduced = HashSet::new();
    for patch in patches {
        if let Patch::Node(node) = patch {
            if !node.metadata_only {
                let current = patch::node(patch::roots(doc, &node.scope)?, &node.path)
                    .ok_or(ScopedHistoryError::NodePathChanged)?;
                let mut current_ids = Vec::new();
                identity::collect_ids(current, &mut current_ids);
                let current_ids: HashSet<_> = current_ids.into_iter().collect();
                let mut pending = vec![node.old.as_ref()];
                while let Some(old) = pending.pop() {
                    if !current_ids.contains(old.id_str()) {
                        introduced.insert(old.id_str().to_owned());
                    }
                    if let Some(children) = old.children() {
                        pending.extend(children.iter().map(std::sync::Arc::as_ref));
                    }
                }
            }
        }
    }
    if introduced.is_empty() {
        return Ok(());
    }
    let mut collision = false;
    identity::walk(&doc.children, |node, _| {
        collision |= introduced.contains(node.id_str())
    });
    if let Some(pages) = &doc.pages {
        for page in pages {
            identity::walk(&page.children, |node, _| {
                collision |= introduced.contains(node.id_str())
            });
        }
    }
    if collision {
        Err(ScopedHistoryError::NodePathChanged)
    } else {
        Ok(())
    }
}
