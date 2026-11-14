//! Compact trusted forest/header history. Complete public snapshots remain separate.
use super::*;
use crate::history_snapshot::SharedNode;
use jian_ops_schema::{node::PenNode, page::PenPage};
use std::{collections::BTreeMap, mem};
mod actor_claim;
mod actor_rebase;
mod boolean;
mod deletion;
pub(crate) use deletion::forecast as deletion_command_bound;
mod clones;
pub(crate) use boolean::forecast as boolean_command_bound;
mod forward;
mod forward_apply;
mod forward_mutation;
mod forward_regions;
mod forward_structure;
mod header;
mod header_map;
mod identity;
mod metadata_changes;
mod patch;
mod splice;
mod splice_exchange;
mod splice_identity;
mod splice_order;
mod targets;
mod topology;
mod topology_move;
mod topology_parent;
mod topology_wrap;
pub(crate) use splice::payload_bound as structural_payload_bound;
pub(crate) use splice::SpliceSpec;
pub(crate) use targets::TargetSpec;
pub(crate) use topology::{TopologyKind, TopologyRequest, TopologySpec};
pub(crate) use topology_parent::Location as TopologyLocation;
mod token_probe;
pub use forward_apply::CommittedMetadataApply;
use header::{HeaderPatch, PendingHeader};
pub(crate) use patch::Scope;
pub use patch::ScopedHistoryError;
use patch::{PageHeader, Patch};
pub(crate) use token_probe::contains as contains_header_token;

#[derive(Debug, Clone, PartialEq)]
pub struct ScopedEditorSnapshot {
    pub(crate) history_tag: super::HistoryTag,
    pub(crate) actor_safe: bool,
    generation: u64,
    page_count: Option<usize>,
    scopes: Vec<Scope>,
    patches: Vec<Patch>,
    page_headers: Vec<PageHeader>,
    header: Option<HeaderPatch>,
    selection: SelectionState,
    pub active_page_index: usize,
    components: SharedComponents,
    app_state_owner: BTreeMap<String, usize>,
    fill_refs: HashMap<NodeId, String>,
    stroke_refs: HashMap<NodeId, String>,
    preserve_authored_geometry: bool,
    pub revision: u64,
}
// New undo fields must be consciously classified here too. Workspace snapshots
// remain on Full while the chooser is active; no mutable document revision is
// used as evidence that public, unversioned content was unchanged.
const _: fn(&EditorSnapshot) = |EditorSnapshot {
                                    history_tag: _,
                                    doc: _,
                                    selection: _,
                                    active_page_index: _,
                                    components: _,
                                    app_state_owner: _,
                                    fill_refs: _,
                                    stroke_refs: _,
                                    preserve_authored_geometry: _,
                                    workspace_directions: _,
                                    revision: _,
                                }| {};

#[derive(Debug)]
pub(crate) struct PendingScopedSnapshot {
    metadata: ScopedEditorSnapshot,
    forests: Vec<CapturedForest>,
    targets: Vec<targets::TargetCheckpoint>,
    targeted: bool,
    header: Option<PendingHeader>,
    clones: Option<clones::PendingClones>,
    boolean: Option<boolean::PendingBoolean>,
    deletion: Option<deletion::PendingDeletion>,
}
#[derive(Debug)]
enum CapturedForest {
    Root(Vec<PenNode>),
    Page { index: usize, page: PenPage },
}
impl PendingScopedSnapshot {
    pub(crate) fn scopes(&self) -> Vec<Scope> {
        self.metadata.scopes.clone()
    }
    pub(crate) fn captures_header(&self) -> bool {
        self.header.is_some()
    }
    pub(crate) fn matches_header(&self, doc: &jian_ops_schema::PenDocument) -> bool {
        self.header
            .as_ref()
            .is_none_or(|header| header.matches(doc))
    }
    #[cfg(test)]
    pub(crate) fn target_checkpoint_counts(&self) -> (usize, usize) {
        targets::checkpoint_counts(&self.targets)
    }
    pub(crate) fn is_targeted(&self) -> bool {
        self.targeted
    }
    pub(crate) fn matches_target_specs(&self, specs: &[TargetSpec]) -> bool {
        targets::matches_specs(&self.targets, specs)
    }
    /// Some means this is the single audited structural transaction. Its
    /// stored request and direct routes avoid a second full-document lookup.
    pub(crate) fn capture_clones(
        state: &EditorState,
        scopes: Vec<Scope>,
        command: &crate::EditorCommand,
    ) -> Option<Self> {
        let scope = scopes.first()?.clone();
        let clones = clones::PendingClones::capture(state, scope, command)?;
        Some(Self {
            metadata: ScopedEditorSnapshot::auxiliary(state, scopes),
            forests: Vec::new(),
            targets: Vec::new(),
            targeted: true,
            header: None,
            clones: Some(clones),
            boolean: None,
            deletion: None,
        })
    }
    pub(crate) fn capture_boolean(
        state: &EditorState,
        scopes: Vec<Scope>,
        command: &crate::EditorCommand,
    ) -> Option<Self> {
        let boolean = boolean::PendingBoolean::capture(state, command)?;
        Some(Self {
            metadata: ScopedEditorSnapshot::auxiliary(state, scopes),
            forests: Vec::new(),
            targets: Vec::new(),
            targeted: true,
            header: None,
            clones: None,
            boolean: Some(boolean),
            deletion: None,
        })
    }
    pub(crate) fn capture_deletion(
        state: &EditorState,
        scopes: Vec<Scope>,
        command: &crate::EditorCommand,
    ) -> Option<Self> {
        let deletion = deletion::PendingDeletion::capture(state, command)?;
        Some(Self {
            metadata: ScopedEditorSnapshot::auxiliary(state, scopes),
            forests: Vec::new(),
            targets: Vec::new(),
            targeted: true,
            header: None,
            clones: None,
            boolean: None,
            deletion: Some(deletion),
        })
    }
    pub(crate) fn matches_topology_request(
        &self,
        state: &EditorState,
        command: &crate::EditorCommand,
    ) -> Option<bool> {
        self.deletion
            .as_ref()
            .map(|deletion| deletion.request_matches(state, command))
            .or_else(|| {
                self.boolean
                    .as_ref()
                    .map(|boolean| boolean.request_matches(state, command))
            })
            .or_else(|| {
                self.clones
                    .as_ref()
                    .map(|clones| clones.request_matches(state, command))
            })
            .or_else(|| targets::matches_topology_request(state, command, &self.targets))
    }
    pub(crate) fn apply_topology(&mut self, state: &mut EditorState) -> Option<bool> {
        if let Some(clones) = &mut self.clones {
            Some(clones.apply_direct(state))
        } else {
            targets::apply_topology(state, &mut self.targets)
        }
    }
    pub(crate) fn matches_targets(&self, state: &EditorState) -> bool {
        self.deletion
            .as_ref()
            .is_none_or(|deletion| deletion.matches(state))
            && self
                .boolean
                .as_ref()
                .is_none_or(|boolean| boolean.matches(state))
            && self
                .clones
                .as_ref()
                .is_none_or(|clones| clones.matches(state))
            && targets::matches(state, &self.targets)
    }
    pub(crate) fn capture_targets(
        state: &EditorState,
        scopes: Vec<Scope>,
        capture_header: bool,
        specs: Vec<TargetSpec>,
    ) -> Result<Self, ScopedHistoryError> {
        Ok(Self {
            metadata: ScopedEditorSnapshot::auxiliary(state, scopes),
            forests: Vec::new(),
            targets: targets::capture(state, specs)?,
            targeted: true,
            clones: None,
            boolean: None,
            deletion: None,
            header: capture_header.then(|| PendingHeader::capture(&state.doc)),
        })
    }
    pub(crate) fn revision(&self) -> u64 {
        self.metadata.revision
    }
    pub(crate) fn restore(&mut self, state: &mut EditorState) -> Result<(), ScopedHistoryError> {
        self.metadata.validate_shape(state)?;
        for forest in mem::take(&mut self.forests) {
            match forest {
                CapturedForest::Root(nodes) => state.doc.children = nodes,
                CapturedForest::Page { index, page } => {
                    state.doc.pages.as_mut().unwrap()[index] = page
                }
            }
        }
        targets::restore(state, std::mem::take(&mut self.targets))?;
        if let Some(clones) = self.clones.take() {
            clones.restore(state)?;
        }
        if let Some(boolean) = self.boolean.take() {
            boolean.restore(state)?;
        }
        if let Some(deletion) = self.deletion.take() {
            deletion.restore(state)?;
        }
        if let Some(header) = self.header.take() {
            header.restore(&mut state.doc);
        }
        state.selection = self.metadata.selection.clone();
        state.ui.active_page_index = self.metadata.active_page_index;
        state.components = self.metadata.components.materialize();
        state.app_state_owner = self.metadata.app_state_owner.clone();
        state.ui.variables.fill_refs = self.metadata.fill_refs.clone();
        state.ui.variables.stroke_refs = self.metadata.stroke_refs.clone();
        state.editor_ui.preserve_authored_geometry = self.metadata.preserve_authored_geometry;
        state.revision = self.metadata.revision;
        state.sync_dirty_flag();
        Ok(())
    }
    pub(crate) fn capture(state: &EditorState, scopes: Vec<Scope>, capture_header: bool) -> Self {
        let forests = scopes
            .iter()
            .map(|scope| match scope {
                Scope::Root => CapturedForest::Root(state.doc.children.clone()),
                Scope::Page { index, .. } => CapturedForest::Page {
                    index: *index,
                    page: state.doc.pages.as_ref().unwrap()[*index].clone(),
                },
            })
            .collect();
        Self {
            metadata: ScopedEditorSnapshot::auxiliary(state, scopes),
            forests,
            targets: Vec::new(),
            targeted: false,
            clones: None,
            boolean: None,
            deletion: None,
            header: capture_header.then(|| PendingHeader::capture(&state.doc)),
        }
    }
    pub(crate) fn apply_metadata_direct(
        &mut self,
        state: &mut EditorState,
        command: &crate::EditorCommand,
        prepared: &crate::command_node_attrs::bulk::Prepared,
    ) -> Option<bool> {
        targets::apply_metadata_direct(state, command, prepared, &mut self.targets)
    }
    pub(crate) fn compact(
        mut self,
        state: &EditorState,
    ) -> Result<ScopedEditorSnapshot, ScopedHistoryError> {
        self.metadata.validate_shape(state)?;
        for forest in self.forests {
            let (scope, old) = match forest {
                CapturedForest::Root(nodes) => (Scope::Root, nodes),
                CapturedForest::Page { index, mut page } => {
                    let current = &state.doc.pages.as_ref().unwrap()[index];
                    let children = mem::take(&mut page.children);
                    let header = page_metadata(current);
                    if !page_metadata_matches(&page, &header) {
                        self.metadata.page_headers.push(PageHeader {
                            index,
                            old: page.clone(),
                        });
                    }
                    (Scope::Page { index, id: page.id }, children)
                }
            };
            patch::compact_forest(
                old,
                patch::roots(&state.doc, &scope)?,
                scope,
                &mut self.metadata.patches,
            );
        }
        self.metadata
            .patches
            .extend(targets::compact(state, self.targets)?);
        if let Some(clones) = self.clones.take() {
            if let Some(clones) = clones.compact(state)? {
                self.metadata.patches.push(Patch::Clones(clones));
            }
        }
        if let Some(boolean) = self.boolean.take() {
            if let Some(patch) = boolean.compact(state)? {
                self.metadata.patches.push(Patch::Clones(patch));
            }
        }
        if let Some(deletion) = self.deletion.take() {
            if let Some(patch) = deletion.compact(state)? {
                self.metadata.patches.push(Patch::Clones(patch));
            }
        }
        self.metadata.header = self.header.map(|header| header.compact(&state.doc));
        let components_unchanged = self.targeted || {
            let current =
                SharedComponents::capture(&state.components, Some(&self.metadata.components));
            Arc::ptr_eq(
                current.components_arc(),
                self.metadata.components.components_arc(),
            ) && current.backing_identity() == self.metadata.components.backing_identity()
        };
        self.metadata.actor_safe = components_unchanged
            && self.metadata.header.is_none()
            && self.metadata.page_headers.is_empty()
            && self.metadata.app_state_owner == state.app_state_owner
            && self.metadata.fill_refs == state.ui.variables.fill_refs
            && self.metadata.stroke_refs == state.ui.variables.stroke_refs
            && self.metadata.preserve_authored_geometry
                == state.editor_ui.preserve_authored_geometry;
        self.metadata.patches.shrink_to_fit();
        self.metadata.page_headers.shrink_to_fit();
        Ok(self.metadata)
    }
}
fn page_metadata(page: &PenPage) -> PenPage {
    PenPage {
        id: page.id.clone(),
        name: page.name.clone(),
        children: Vec::new(),
        background_color: page.background_color.clone(),
        state: page.state.clone(),
        lifecycle: page.lifecycle.clone(),
    }
}
fn page_metadata_matches(a: &PenPage, b: &PenPage) -> bool {
    a == b
        && crate::history_snapshot::metadata_sensitivity(a)
            == crate::history_snapshot::metadata_sensitivity(b)
}
impl ScopedEditorSnapshot {
    /// A Full restore has just installed the exact checkpoint preceding this
    /// contiguous scoped history segment. Keep the live edit generation
    /// monotonic without invalidating that segment or mutating cloned editors.
    pub(super) fn rebase_generation(snapshot: &mut Arc<Self>, generation: u64) {
        if snapshot.generation != generation {
            Arc::make_mut(snapshot).generation = generation;
        }
    }
    fn auxiliary(state: &EditorState, scopes: Vec<Scope>) -> Self {
        Self {
            history_tag: super::HistoryTag::capture(state.history.actor()),
            actor_safe: false,
            generation: state.document_generation,
            page_count: state.doc.pages.as_ref().map(Vec::len),
            scopes,
            patches: Vec::new(),
            page_headers: Vec::new(),
            header: None,
            selection: state.selection.clone(),
            active_page_index: state.ui.active_page_index,
            components: SharedComponents::capture(
                &state.components,
                state
                    .history
                    .future
                    .back()
                    .or_else(|| state.history.past.back())
                    .map(|entry| match entry {
                        HistoryEntry::Full(snapshot) => &snapshot.components,
                        HistoryEntry::Scoped(snapshot) => snapshot.components(),
                    }),
            ),
            app_state_owner: state.app_state_owner.clone(),
            fill_refs: state.ui.variables.fill_refs.clone(),
            stroke_refs: state.ui.variables.stroke_refs.clone(),
            preserve_authored_geometry: state.editor_ui.preserve_authored_geometry,
            revision: state.revision,
        }
    }
    pub(crate) fn has_canonical_changes(&self) -> bool {
        !self.patches.is_empty() || !self.page_headers.is_empty() || self.header.is_some()
    }
    pub(crate) fn scopes(&self) -> Vec<Scope> {
        self.scopes.clone()
    }
    pub(crate) fn captures_header(&self) -> bool {
        self.header.is_some()
    }
    pub fn components(&self) -> &SharedComponents {
        &self.components
    }
    pub fn retained_roots(&self) -> impl Iterator<Item = &Arc<SharedNode>> {
        self.patches.iter().flat_map(Patch::retained_roots)
    }
    /// Includes every owned non-SharedNode allocation. Shared component roots
    /// and backing are exposed separately for identity-aware ledger accounting.
    pub fn storage_bytes(&self) -> usize {
        fn id(id: &NodeId) -> usize {
            64 + 4 * id.as_str().len()
        }
        fn refs(map: &HashMap<NodeId, String>) -> usize {
            128 + map.capacity() * (mem::size_of::<(NodeId, String)>() + 16)
                + map
                    .iter()
                    .map(|(key, value)| id(key) + value.capacity())
                    .sum::<usize>()
        }
        let mut total = mem::size_of::<Self>()
            + 2 * mem::size_of::<usize>()
            + self.scopes.capacity() * mem::size_of::<Scope>()
            + self.patches.capacity() * mem::size_of::<Patch>()
            + self.page_headers.capacity() * mem::size_of::<PageHeader>()
            + self.patches.iter().map(Patch::storage_bytes).sum::<usize>()
            + self
                .scopes
                .iter()
                .map(|scope| match scope {
                    Scope::Root => 0,
                    Scope::Page { id, .. } => id.capacity(),
                })
                .sum::<usize>()
            + self.selection.set.capacity() * mem::size_of::<NodeId>()
            + id(&self.selection.anchor)
            + self.selection.set.iter().map(id).sum::<usize>()
            + refs(&self.fill_refs)
            + refs(&self.stroke_refs)
            + 1024
            + self.app_state_owner.len() * 256
            + self
                .app_state_owner
                .keys()
                .map(String::capacity)
                .sum::<usize>();
        total = total.saturating_add(self.header.as_ref().map_or(0, HeaderPatch::storage_bytes));
        struct Count(usize);
        impl std::io::Write for Count {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0 = self.0.saturating_add(bytes.len());
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        for header in &self.page_headers {
            let mut count = Count(0);
            if serde_json::to_writer(&mut count, &header.old).is_err() {
                return usize::MAX;
            }
            total = total.saturating_add(count.0.saturating_mul(4));
        }
        total
    }
    fn validate_shape(&self, state: &EditorState) -> Result<(), ScopedHistoryError> {
        if state.document_generation != self.generation {
            return Err(ScopedHistoryError::DocumentGenerationChanged {
                expected: self.generation,
                actual: state.document_generation,
            });
        }
        if state.doc.pages.as_ref().map(Vec::len) != self.page_count {
            return Err(ScopedHistoryError::PageShapeChanged);
        }
        for scope in &self.scopes {
            patch::roots(&state.doc, scope)?;
        }
        Ok(())
    }
    /// All destinations are validated before any write. One historical side
    /// becomes the reverse entry; no permanent before/after document pair.
    pub(crate) fn exchange(&mut self, state: &mut EditorState) -> Result<(), ScopedHistoryError> {
        self.validate_shape(state)?;
        let catalog = splice_exchange::Catalog::new(&state.history, &self.patches);
        let splices =
            splice_exchange::Prepared::capture(&state.doc, &self.patches, &catalog, false)?;
        if let Some(prepared) = &splices {
            prepared.rebase(&mut self.patches);
        }
        for patch in &self.patches {
            patch.validate(&state.doc)?;
        }
        if !self
            .header
            .as_ref()
            .is_none_or(|header| header.validate(&state.doc))
        {
            return Err(ScopedHistoryError::DocumentHeaderChanged);
        }
        let reverse = Self::auxiliary(state, self.scopes.clone());
        let mut reordered = patch::Reorders::default();
        if let Some(splices) = splices {
            splices.exchange(&mut state.doc, &mut self.patches);
        } else {
            for patch in &mut self.patches {
                patch.exchange(&mut state.doc, &mut reordered);
            }
            for patch in &mut self.patches {
                patch.rebase(&reordered);
            }
        }
        for header in &mut self.page_headers {
            let current = &mut state.doc.pages.as_mut().unwrap()[header.index];
            let children = mem::take(&mut current.children);
            mem::swap(current, &mut header.old);
            current.children = children;
        }
        if let Some(header) = &mut self.header {
            header.exchange(&mut state.doc);
        }
        state.selection = mem::replace(&mut self.selection, reverse.selection);
        state.ui.active_page_index =
            mem::replace(&mut self.active_page_index, reverse.active_page_index);
        state.components = self.components.materialize();
        self.components = reverse.components;
        state.app_state_owner = mem::replace(&mut self.app_state_owner, reverse.app_state_owner);
        state.ui.variables.fill_refs = mem::replace(&mut self.fill_refs, reverse.fill_refs);
        state.ui.variables.stroke_refs = mem::replace(&mut self.stroke_refs, reverse.stroke_refs);
        state.editor_ui.preserve_authored_geometry = mem::replace(
            &mut self.preserve_authored_geometry,
            reverse.preserve_authored_geometry,
        );
        state.revision = mem::replace(&mut self.revision, reverse.revision);
        state.sync_dirty_flag();
        Ok(())
    }
}
