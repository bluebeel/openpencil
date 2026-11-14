//! Bounded forward STATE data from completed exact rollback patches.
//! Existing structural planners project identity orders; no command replay.

use super::{
    forward_structure::{self, Budget, Children},
    patch, ScopedEditorSnapshot,
};
use crate::{EditorState, PenNodeExt};
use jian_ops_schema::node::PenNode;
use serde::Serialize;
use std::io::{self, Write};

const MAX_OPERATIONS: usize = 4096;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NodeMetadata<'a> {
    op: &'static str,
    page: Option<Page<'a>>,
    path: &'a [usize],
    expected_id: &'a str,
    children_present: bool,
    value: crate::history_snapshot::BorrowedNodeMetadata<'a>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NodeValue<'a> {
    op: &'static str,
    page: Option<Page<'a>>,
    path: &'a [usize],
    expected_id: &'a str,
    value: &'a PenNode,
}
#[derive(Serialize)]
pub(super) struct Page<'a> {
    index: usize,
    id: &'a str,
}
impl<'a> Page<'a> {
    pub(super) fn for_scope(scope: &'a patch::Scope) -> Option<Self> {
        match scope {
            patch::Scope::Root => None,
            patch::Scope::Page { index, id } => Some(Self { index: *index, id }),
        }
    }
}
#[derive(Serialize)]
#[serde(untagged)]
enum Operation<'a> {
    Children(Children<'a>),
    Metadata(NodeMetadata<'a>),
    Node(NodeValue<'a>),
}

impl ScopedEditorSnapshot {
    /// Complete bounded post-state for exactly captured node/structural edits.
    /// Broad/global/page-header or unsupported forms require explicit resync.
    pub(crate) fn forward_metadata_json(
        &self,
        state: &EditorState,
        limit: usize,
    ) -> Option<Vec<u8>> {
        if self.header.is_some()
            || !self.page_headers.is_empty()
            || self.patches.len() > MAX_OPERATIONS
        {
            return None;
        }
        self.validate_shape(state).ok()?;
        let mut budget = Budget::new(limit);
        let mut children = Vec::new();
        let mut nodes = Vec::new();
        if self
            .patches
            .iter()
            .any(|edit| edit.splices().next().is_some())
        {
            // Reuse the actual stable identity/anchor plan. Numeric offsets in
            // sealed duplicate/paste history are deliberately not authoritative.
            if self
                .patches
                .iter()
                .any(|edit| edit.splices().next().is_none())
            {
                return None;
            }
            let catalog = super::splice_exchange::Catalog::new(&state.history, &self.patches);
            let prepared = super::splice_exchange::Prepared::capture(
                &state.doc,
                &self.patches,
                &catalog,
                false,
            )
            .ok()??;
            children = prepared.forward_children(&state.doc, &mut budget)?;
        } else {
            for edit in &self.patches {
                edit.validate(&state.doc).ok()?;
                match edit {
                    patch::Patch::Node(p) => {
                        let current =
                            patch::node(patch::roots(&state.doc, &p.scope).ok()?, &p.path)?;
                        nodes.push((p, current));
                    }
                    patch::Patch::Forest(p) => {
                        children.push(p.forward_children(&state.doc, &mut budget)?)
                    }
                    patch::Patch::Order(p) => {
                        children.push(p.forward_children(&state.doc, &mut budget)?)
                    }
                    patch::Patch::Topology(super::topology::TopologyPatch::Move(p)) => {
                        children.extend(p.forward_children(&state.doc, &mut budget)?)
                    }
                    patch::Patch::Topology(super::topology::TopologyPatch::Wrap(p)) => {
                        children.push(p.forward_children(&state.doc, &mut budget)?)
                    }
                    _ => return None,
                }
            }
        }
        // Complete subtree/list replacements cover their descendants. Emit only
        // the outer state so no client has to apply stale intermediate routes.
        let mut full = nodes
            .iter()
            .filter(|(p, _)| !p.metadata_only)
            .copied()
            .collect::<Vec<_>>();
        full.sort_by_key(|(p, _)| p.path.len());
        children.retain(|child| {
            !full
                .iter()
                .any(|(p, _)| p.scope == child.scope && child.parent.starts_with(&p.path))
        });
        children.sort_by_key(|child| child.parent.len());
        let mut outer: Vec<Children<'_>> = Vec::new();
        for child in children {
            if let Some(parent) = outer.iter().find(|parent| {
                parent.scope == child.scope && child.parent.starts_with(&parent.parent)
            }) {
                if parent.parent == child.parent
                    && (parent.expected_ids != child.expected_ids
                        || parent.expected_present != child.expected_present)
                {
                    return None;
                }
                continue;
            }
            outer.push(child);
        }
        let mut whole = Vec::new();
        for node in full {
            if outer
                .iter()
                .any(|parent| parent.covers(&node.0.scope, &node.0.path))
                || whole.iter().any(|(p, _): &(&patch::NodePatch, &PenNode)| {
                    p.scope == node.0.scope && node.0.path.starts_with(&p.path)
                })
            {
                continue;
            }
            if !forward_structure::bounded_tree(std::slice::from_ref(node.1)) {
                return None;
            }
            whole.push(node);
        }
        let mut operations = outer
            .into_iter()
            .map(Operation::Children)
            .collect::<Vec<_>>();
        for (p, current) in &whole {
            operations.push(Operation::Node(NodeValue {
                op: "replace-node",
                page: Page::for_scope(&p.scope),
                path: &p.path,
                expected_id: p.old.id_str(),
                value: current,
            }));
        }
        for (p, current) in nodes {
            if !p.metadata_only || operations.iter().any(|op| matches!(op, Operation::Children(parent) if parent.covers(&p.scope, &p.path)))
                || whole.iter().any(|(parent,_)| parent.scope == p.scope && p.path.starts_with(&parent.path)) { continue; }
            operations.push(Operation::Metadata(NodeMetadata {
                op: "replace-node-metadata",
                page: Page::for_scope(&p.scope),
                path: &p.path,
                expected_id: p.old.id_str(),
                children_present: current.children().is_some(),
                value: crate::history_snapshot::BorrowedNodeMetadata(current),
            }));
        }
        if operations.len() > MAX_OPERATIONS {
            return None;
        }
        let mut writer = Limited {
            bytes: Vec::new(),
            limit,
        };
        // Force canonical inline serialization even if a caller has an image
        // externalization scope active. Every subtree remains self-contained.
        jian_ops_schema::image_table::without_save_scope(|| {
            serde_json::to_writer(&mut writer, &operations)
        })
        .ok()?;
        Some(writer.bytes)
    }
}

struct Limited {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for Limited {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("forward metadata exceeds feed allowance"));
        }
        // Reserve exact growth so retained backing never silently doubles the
        // public payload bound. No complete header/tree is cloned first.
        if self.bytes.capacity().saturating_sub(self.bytes.len()) < bytes.len() {
            let required = self.bytes.len() + bytes.len();
            let capacity = required
                .checked_next_power_of_two()
                .unwrap_or(required)
                .max(4096)
                .min(self.limit);
            self.bytes.reserve_exact(capacity - self.bytes.len());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
