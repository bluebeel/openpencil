use std::{
    collections::{BTreeMap, VecDeque},
    io::Write,
    sync::Arc,
};

use jian_ops_schema::PenDocument;

use crate::{
    apply::apply_txn_owned,
    canonical_node_hash,
    diff_fields::{apply_changes_to_node, node_map},
    diff_index::TreeIndex,
    ApplyContext, ApplyLimits, ClientOpId, CollabOp, CollabTxn, CommitSeq, EditChanges, FieldValue,
    NodeFieldChange, PageRef, PeerId, SessionError, SupportedNodeField,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct OperationKey {
    peer_id: Arc<str>,
    counter: u64,
}

impl OperationKey {
    pub(crate) fn from_client_op_id(id: &ClientOpId) -> Self {
        Self {
            peer_id: Arc::from(id.peer_id.as_ref()),
            counter: id.local_counter,
        }
    }

    fn client_op_id(&self) -> ClientOpId {
        ClientOpId {
            peer_id: PeerId::from(self.peer_id.to_string()),
            local_counter: self.counter,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PageKey {
    DocumentRoot,
    Page(String),
}

impl From<&PageRef> for PageKey {
    fn from(page: &PageRef) -> Self {
        match page {
            PageRef::DocumentRoot => Self::DocumentRoot,
            PageRef::PageId(id) => Self::Page(id.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FieldKey {
    page: PageKey,
    node_id: String,
    field: SupportedNodeField,
}

impl FieldKey {
    fn from_change(change: &NodeFieldChange) -> Self {
        Self {
            page: PageKey::from(&change.page),
            node_id: change.node_id.clone(),
            field: change.field,
        }
    }
}

#[derive(Clone)]
pub(crate) struct UndoRecord {
    pub(crate) author_peer_id: PeerId,
    pub(crate) seq: CommitSeq,
    pub(crate) changes: Arc<[NodeFieldChange]>,
    pub(crate) undone: bool,
    weight: usize,
}

pub(crate) struct UndoSelection {
    pub(crate) candidate: PenDocument,
    pub(crate) reverted_fields: Vec<FieldKey>,
    pub(crate) total_fields: usize,
}

#[derive(Default)]
pub(crate) struct UndoLedger {
    records: BTreeMap<OperationKey, UndoRecord>,
    order: VecDeque<OperationKey>,
    last_writer: BTreeMap<FieldKey, OperationKey>,
    field_refs: BTreeMap<FieldKey, usize>,
    bytes: usize,
}

impl UndoLedger {
    pub(crate) fn record_commit(
        &mut self,
        client_op_id: &ClientOpId,
        seq: CommitSeq,
        changes: &EditChanges,
        undoable: bool,
        max_entries: usize,
        max_bytes: usize,
    ) {
        let EditChanges::Property(changes) = changes else {
            return;
        };
        let key = OperationKey::from_client_op_id(client_op_id);
        let weight = record_weight(changes);
        if undoable && weight <= max_bytes {
            let record = UndoRecord {
                author_peer_id: client_op_id.peer_id.clone(),
                seq,
                changes: Arc::from(changes.as_slice()),
                undone: false,
                weight,
            };
            if let Some(previous) = self.records.insert(key.clone(), record) {
                if !previous.undone {
                    self.remove_field_refs(&previous.changes);
                }
                self.bytes = self.bytes.saturating_sub(previous.weight);
                self.order.retain(|entry| entry != &key);
            }
            self.add_field_refs(changes);
            self.bytes = self.bytes.saturating_add(weight);
            self.order.push_back(key.clone());
        }
        for change in changes {
            let field = FieldKey::from_change(change);
            if self.field_refs.contains_key(&field) {
                self.last_writer.insert(field, key.clone());
            }
        }
        while self.records.len() > max_entries || self.bytes > max_bytes {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(removed) = self.records.remove(&oldest) {
                if !removed.undone {
                    self.remove_field_refs(&removed.changes);
                }
                self.bytes = self.bytes.saturating_sub(removed.weight);
            }
        }
    }

    fn add_field_refs(&mut self, changes: &[NodeFieldChange]) {
        for change in changes {
            let field = FieldKey::from_change(change);
            *self.field_refs.entry(field).or_insert(0) += 1;
        }
    }

    fn remove_field_refs(&mut self, changes: &[NodeFieldChange]) {
        for change in changes {
            let field = FieldKey::from_change(change);
            let remove = if let Some(count) = self.field_refs.get_mut(&field) {
                *count = count.saturating_sub(1);
                *count == 0
            } else {
                false
            };
            if remove {
                self.field_refs.remove(&field);
                self.last_writer.remove(&field);
            }
        }
    }

    pub(crate) fn record(&self, target: &ClientOpId) -> Option<UndoRecord> {
        self.records
            .get(&OperationKey::from_client_op_id(target))
            .cloned()
    }

    pub(crate) fn targets_for_peer(&self, peer_id: &PeerId) -> Vec<ClientOpId> {
        self.order
            .iter()
            .filter_map(|key| {
                let record = self.records.get(key)?;
                (!record.undone && record.author_peer_id == *peer_id).then(|| key.client_op_id())
            })
            .collect()
    }

    pub(crate) fn mark_undone(&mut self, target: &ClientOpId) {
        let key = OperationKey::from_client_op_id(target);
        let changes = self.records.get_mut(&key).and_then(|record| {
            if record.undone {
                None
            } else {
                record.undone = true;
                Some(record.changes.clone())
            }
        });
        if let Some(changes) = changes {
            self.remove_field_refs(&changes);
        }
    }

    pub(crate) fn build_selection(
        &self,
        document: &PenDocument,
        target: &ClientOpId,
        record: &UndoRecord,
        limits: ApplyLimits,
    ) -> Result<UndoSelection, SessionError> {
        let target_key = OperationKey::from_client_op_id(target);
        let index = TreeIndex::build(document).map_err(SessionError::UndoDiff)?;
        let mut reverse_by_node: BTreeMap<String, Vec<NodeFieldChange>> = BTreeMap::new();
        let mut reverted_fields = Vec::new();
        let mut fields_by_node = BTreeMap::new();

        for change in record.changes.iter() {
            let field_key = FieldKey::from_change(change);
            if self.last_writer.get(&field_key) != Some(&target_key) {
                continue;
            }
            let Some(entry) = index.entry(&change.node_id) else {
                continue;
            };
            if entry.page != change.page {
                continue;
            }
            if !fields_by_node.contains_key(change.node_id.as_str()) {
                fields_by_node.insert(
                    change.node_id.as_str(),
                    node_map(entry.node).map_err(SessionError::UndoDiff)?,
                );
            }
            let fields = &fields_by_node[change.node_id.as_str()];
            let matches = match (&change.desired, fields.get(change.field.wire_name())) {
                (FieldValue::Missing, None) => true,
                (FieldValue::Value(expected), Some(actual)) => expected == actual,
                _ => false,
            };
            if !matches {
                continue;
            }
            reverse_by_node
                .entry(change.node_id.clone())
                .or_default()
                .push(NodeFieldChange {
                    page: change.page.clone(),
                    node_id: change.node_id.clone(),
                    field: change.field,
                    before: change.desired.clone(),
                    desired: change.before.clone(),
                });
            reverted_fields.push(field_key);
        }

        let mut candidate = document.clone();
        for node_id in &index.preorder {
            let Some(changes) = reverse_by_node.remove(node_id) else {
                continue;
            };
            // Parent-before-descendant header edits preserve this original
            // target subtree until its own turn; no candidate-wide reindex.
            let entry = index.entry(node_id).ok_or(SessionError::UndoDiff(
                crate::DiffError::StructuralPlanningMismatch,
            ))?;
            let replacement =
                apply_changes_to_node(entry.node, &changes).map_err(SessionError::UndoDiff)?;
            let operation = CollabOp::ReplaceExact {
                page: entry.page.clone(),
                node_id: node_id.clone(),
                expected_hash: canonical_node_hash(entry.node)?,
                node: replacement,
            };
            let mut context = ApplyContext::standalone_trusted();
            context.limits = limits;
            candidate = apply_txn_owned(candidate, &CollabTxn::new(vec![operation]), &context)
                .map_err(SessionError::UndoApply)?;
        }
        debug_assert!(reverse_by_node.is_empty());
        Ok(UndoSelection {
            candidate,
            reverted_fields,
            total_fields: record.changes.len(),
        })
    }
}

fn record_weight(changes: &[NodeFieldChange]) -> usize {
    changes.iter().fold(64_usize, |weight, change| {
        weight
            .saturating_add(change.node_id.len())
            .saturating_add(page_weight(&change.page))
            .saturating_add(value_weight(&change.before))
            .saturating_add(value_weight(&change.desired))
            .saturating_add(32)
    })
}

fn page_weight(page: &PageRef) -> usize {
    match page {
        PageRef::DocumentRoot => 1,
        PageRef::PageId(id) => id.len(),
    }
}

fn value_weight(value: &FieldValue) -> usize {
    match value {
        FieldValue::Missing => 1,
        FieldValue::Value(value) => {
            let mut count = ByteCount(0);
            serde_json::to_writer(&mut count, value).map_or(usize::MAX, |_| count.0)
        }
    }
}

struct ByteCount(usize);
impl Write for ByteCount {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
