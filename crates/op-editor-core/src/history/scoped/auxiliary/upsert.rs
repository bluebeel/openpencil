//! Audited conversion upserts: canonical tree destination, one ledger unit,
//! and (for components) one independently rebased catalog entry.
use super::super::{Scope, SpliceSpec, TargetSpec};
use super::{bulk::conversion, catalog, exact, EditorState, Patch, ScopedHistoryError, C};
use crate::{
    conversion::{PreparedUpsert, UpsertDestination},
    NodeId, SharedNode,
};
use jian_ops_schema::{conversion::ConversionKind, node::PenNode};
use std::sync::Arc;
mod page;
const LIMIT: usize = crate::history_snapshot::COMMAND_CAPTURE_ALLOWANCE;
#[derive(Debug)]
struct Request {
    kind: ConversionKind,
    key: String,
    name: Option<String>,
    root: Arc<SharedNode>,
    source_path: Option<String>,
    source_hash: Option<String>,
}
fn request(
    command: &C,
) -> Option<(
    ConversionKind,
    &str,
    Option<&str>,
    &PenNode,
    &Option<String>,
    &Option<String>,
)> {
    match command {
        C::UpsertComponent {
            key,
            name,
            root,
            source_path,
            source_hash,
        } => Some((
            ConversionKind::Component,
            key.trim(),
            Some(name.trim()),
            root,
            source_path,
            source_hash,
        )),
        C::UpsertScreen {
            key,
            root,
            source_path,
            source_hash,
        } => Some((
            ConversionKind::Screen,
            key.trim(),
            None,
            root,
            source_path,
            source_hash,
        )),
        _ => None,
    }
}
pub(super) fn supports(command: &C) -> bool {
    request(command).is_some()
}
fn add<T: serde::Serialize + ?Sized>(value: &T, bytes: &mut usize) -> Option<()> {
    *bytes = bytes.checked_add(
        crate::history_snapshot::command_capture_value_bytes(&value, LIMIT.checked_sub(*bytes)?)
            .ok()?,
    )?;
    (*bytes <= LIMIT).then_some(())
}
pub(super) fn forecast(state: &EditorState, command: &C) -> Option<usize> {
    let (kind, key, name, root, source_path, source_hash) = request(command)?;
    if key.is_empty()
        || name.is_some_and(str::is_empty)
        || state.runtime_allocator_depth != 0
        || state.undo_capture_suspended
        || state.editor_ui.workspace.active
        || state.instance_write_virtual_anchor.is_some()
    {
        return None;
    }
    let mut bytes = 128 * 1024usize;
    add(&(key, name, source_path, source_hash), &mut bytes)?;
    let payload =
        crate::history_snapshot::command_capture_node_bytes(root, false, LIMIT.checked_sub(bytes)?)
            .ok()?;
    // Prepared tree, source witness, remapped witness, expected history and
    // allocation scratch coexist only for this bounded incoming root.
    bytes = bytes.checked_add(payload.checked_mul(8)?)?;
    conversion::forecast_kind(&state.doc, kind, key, &mut bytes, LIMIT)?;
    let mut entries = state
        .doc
        .conversion
        .iter()
        .flat_map(|c| &c.entries)
        .filter(|e| e.kind == kind && e.key == key);
    if let Some(entry) = entries.next() {
        if entries.next().is_some() {
            return None;
        }
        let (node, _) = crate::component_backing::find_node_and_location(
            &state.doc,
            entry.node_id.as_deref()?,
        )?;
        bytes = bytes.checked_add(
            crate::history_snapshot::command_capture_node_bytes(
                node,
                false,
                LIMIT.checked_sub(bytes)?,
            )
            .ok()?
            .checked_mul(3)?,
        )?;
    }
    for entry in &state.components.components {
        bytes = bytes.checked_add(entry.id.as_str().len().checked_mul(8)?.checked_add(128)?)?;
    }
    if kind == ConversionKind::Component {
        // COW registry headers/backing and stable catalog order, never an owned
        // external prototype graph. Preparation proves document backing.
        for entry in &state.components.components {
            if !state.components.is_document_backed(&entry.id) {
                return None;
            }
            bytes = bytes.checked_add(
                crate::history_snapshot::command_capture_node_bytes(
                    &entry.root,
                    true,
                    LIMIT.checked_sub(bytes)?,
                )
                .ok()?
                .checked_mul(3)?,
            )?;
            add(&(&entry.id.as_str(), &entry.name), &mut bytes)?;
        }
        bytes = bytes.checked_add(
            state
                .components
                .document_backing()
                .capacity()
                .checked_mul(256)?,
        )?;
        for location in state.components.document_backing().values() {
            bytes = bytes.checked_add(location.child_path.len().checked_mul(64)?)?;
        }
    }
    for page in state.doc.pages.iter().flatten() {
        bytes = bytes.checked_add(page.id.len().checked_mul(16)?.checked_add(1024)?)?;
    }
    // A root append can relocate its sibling Vec; charge slots without cloning
    // their payload. Replacement does not own unrelated sibling trees.
    bytes = bytes.checked_add(
        state
            .doc
            .children
            .capacity()
            .checked_mul(std::mem::size_of::<PenNode>() * 2)?,
    )?;
    for page in state.doc.pages.iter().flatten() {
        bytes = bytes.checked_add(
            page.children
                .capacity()
                .checked_mul(std::mem::size_of::<PenNode>() * 2)?,
        )?;
    }
    (bytes <= LIMIT).then_some(bytes)
}
#[derive(Debug)]
pub(super) struct Pending {
    request: Request,
    prepared: PreparedUpsert,
    expected: Arc<SharedNode>,
    ledger: conversion::Pending,
    old: Option<catalog::Entry>,
    anchor: super::super::splice_order::Anchors,
    order: Vec<NodeId>,
}
pub(super) struct Plan {
    pub pending: Pending,
    pub scopes: Vec<Scope>,
    pub targets: Vec<TargetSpec>,
}
impl Pending {
    pub(super) fn capture(state: &EditorState, command: &C) -> Option<Plan> {
        let bytes = forecast(state, command)?;
        let prepared = PreparedUpsert::prepare(state, command)?;
        let (kind, key, name, root, source_path, source_hash) = request(command)?;
        let ledger = conversion::Pending::capture_kind(&state.doc, kind, key)?;
        let position = catalog::position(state, &prepared.id).ok()?;
        let old = catalog::Entry::capture(state, &prepared.id, None).ok()?;
        let anchor = position.map_or_else(
            || catalog::append_anchors(state, &prepared.id),
            |i| catalog::anchors(state, &prepared.id, i),
        );
        let targets = match &prepared.destination {
            UpsertDestination::Replace { scope, path } => vec![TargetSpec {
                scope: scope.clone(),
                path: path.clone(),
                subtree: true,
                splice: None,
                topology: None,
            }],
            UpsertDestination::Append { scope, index } => vec![TargetSpec {
                scope: scope.clone(),
                path: Box::new([]),
                subtree: false,
                splice: Some(SpliceSpec {
                    index: *index,
                    remove: 0,
                    insert_limit: 1,
                    forecast: bytes,
                }),
                topology: None,
            }],
            UpsertDestination::NewPage { .. } => Vec::new(),
        };
        let scopes = targets.iter().map(|t| t.scope.clone()).collect();
        let expected = SharedNode::capture(prepared.root(), None);
        let request = Request {
            kind,
            key: key.into(),
            name: name.map(str::to_owned),
            root: SharedNode::capture(root, None),
            source_path: source_path.clone(),
            source_hash: source_hash.clone(),
        };
        let order = if kind == ConversionKind::Component {
            state
                .components
                .components
                .iter()
                .map(|c| c.id.clone())
                .collect()
        } else {
            Vec::new()
        };
        Some(Plan {
            pending: Self {
                request,
                prepared,
                expected,
                ledger,
                old,
                anchor,
                order,
            },
            scopes,
            targets,
        })
    }
    pub(super) fn request_matches(&self, command: &C) -> bool {
        request(command).is_some_and(|(kind, key, name, root, path, hash)| {
            self.request.kind == kind
                && self.request.key == key
                && self.request.name.as_deref() == name
                && exact(&self.request.source_path, path)
                && exact(&self.request.source_hash, hash)
                && self.request.root.equals_node(root)
        })
    }
    pub(super) fn matches_header(&self, doc: &jian_ops_schema::PenDocument) -> bool {
        self.ledger.matches(doc)
    }
    pub(super) fn matches(&self, state: &EditorState) -> bool {
        self.ledger.matches(&state.doc)
            && self.prepared.matches_destination(state)
            && (self.request.kind != ConversionKind::Component
                || (state
                    .components
                    .components
                    .iter()
                    .map(|c| &c.id)
                    .eq(&self.order)
                    && match (&self.old, catalog::position(state, &self.prepared.id)) {
                        (Some(old), Ok(Some(at))) => old.matches(state, at),
                        (None, Ok(None)) => !state
                            .components
                            .document_backing()
                            .contains_key(&self.prepared.id),
                        _ => false,
                    }))
    }
    pub(super) fn apply(&mut self, state: &mut EditorState, command: &C) -> bool {
        self.request_matches(command) && self.matches(state) && self.prepared.commit(state)
    }
    pub(super) fn prepare_page(
        &self,
        state: &EditorState,
        snapshot: &mut super::super::ScopedEditorSnapshot,
    ) -> Result<(), ScopedHistoryError> {
        page::prepare(self, state, snapshot)
    }
    pub(super) fn compact(
        self,
        state: &EditorState,
        patches: &mut Vec<Patch>,
    ) -> Result<(), ScopedHistoryError> {
        if self.request.kind == ConversionKind::Component {
            if let Some(patch) =
                catalog::Patch::compact(state, self.prepared.id, self.old, self.anchor)?
            {
                patches.push(Patch::Catalog(patch));
            }
        }
        if let Some(patch) = self.ledger.compact(&state.doc)? {
            patches.push(Patch::ConversionUnit(patch));
        }
        Ok(())
    }
}
