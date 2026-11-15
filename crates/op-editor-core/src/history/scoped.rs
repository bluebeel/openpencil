//! Compact trusted forest/header history. Complete public snapshots remain separate.
use super::*;
use crate::history_snapshot::SharedNode;
use jian_ops_schema::{node::PenNode, page::PenPage};
use std::{collections::BTreeMap, mem};
mod actor_claim;
mod auxiliary;
mod component_local;
pub(crate) use auxiliary::{
    forecast as auxiliary_command_forecast, supports as auxiliary_command_supported,
};
mod actor_rebase;
mod boolean;
mod deletion;
pub(crate) use deletion::forecast as deletion_command_bound;
mod clones;
mod cohort;
pub(crate) use boolean::forecast as boolean_command_bound;
mod forward;
mod forward_apply;
mod forward_catalog;
mod forward_mutation;
mod forward_regions;
mod forward_structure;
mod header;
mod header_map;
mod identity;
mod incremental;
mod incremental_regions;
mod metadata_changes;
mod pages;
mod patch;
mod ref_target;
mod restore;
mod splice;
mod splice_exchange;
mod splice_identity;
mod splice_order;
mod targets;
mod topology;
mod topology_move;
mod topology_parent;
mod topology_wrap;
mod upsert_lifecycle;
mod variable_refs;
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
    page_patch: Option<pages::Patch>,
    active_page_id: Option<String>,
    scopes: Vec<Scope>,
    patches: Vec<Patch>,
    auxiliary_patches: Vec<auxiliary::Patch>,
    page_headers: Vec<PageHeader>,
    header: Option<HeaderPatch>,
    selection: SelectionState,
    pub active_page_index: usize,
    components: SharedComponents,
    captures_components: bool,
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
    auxiliary: Option<auxiliary::Pending>,
}
#[derive(Debug)]
enum CapturedForest {
    Root(Vec<PenNode>),
    Regions(incremental_regions::PendingRegions),
    Page { index: usize, page: PenPage },
}
impl PendingScopedSnapshot {
    pub(crate) fn scopes(&self) -> Vec<Scope> {
        self.metadata.scopes.clone()
    }
    pub(crate) fn captures_header(&self) -> bool {
        self.header.is_some()
            || self
                .auxiliary
                .as_ref()
                .is_some_and(|patch| patch.is_header())
    }
    pub(crate) fn matches_header(&self, doc: &jian_ops_schema::PenDocument) -> bool {
        self.header
            .as_ref()
            .is_none_or(|header| header.matches(doc))
            && self
                .auxiliary
                .as_ref()
                .is_none_or(|patch| patch.matches_header(doc))
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
        captures_components: bool,
    ) -> Option<Self> {
        let scope = scopes.first()?.clone();
        let clones = clones::PendingClones::capture(state, scope, command)?;
        Some(Self {
            metadata: ScopedEditorSnapshot::auxiliary_with_components(
                state,
                scopes,
                captures_components,
            ),
            forests: Vec::new(),
            targets: Vec::new(),
            targeted: true,
            header: None,
            clones: Some(clones),
            boolean: None,
            deletion: None,
            auxiliary: None,
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
            auxiliary: None,
        })
    }
    pub(crate) fn deletion_preserves_components(state: &EditorState) -> bool {
        deletion::catalog_preserving(state)
    }
    pub(crate) fn capture_deletion(
        state: &EditorState,
        scopes: Vec<Scope>,
        command: &crate::EditorCommand,
        captures_components: bool,
    ) -> Option<Self> {
        let deletion = deletion::PendingDeletion::capture(state, command)?;
        Some(Self {
            metadata: ScopedEditorSnapshot::auxiliary_with_components(
                state,
                scopes,
                captures_components,
            ),
            forests: Vec::new(),
            targets: Vec::new(),
            targeted: true,
            header: None,
            clones: None,
            boolean: None,
            deletion: Some(deletion),
            auxiliary: None,
        })
    }
    pub(crate) fn auxiliary_request_matches(
        &self,
        state: &EditorState,
        command: &crate::EditorCommand,
    ) -> Option<bool> {
        self.auxiliary.as_ref().map(|patch| {
            patch.request_matches(command)
                && patch.matches(state)
                && targets::matches(state, &self.targets)
        })
    }
    pub(crate) fn captures_variable_key(&self) -> bool {
        self.auxiliary
            .as_ref()
            .is_some_and(|patch| patch.is_header())
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
        self.auxiliary
            .as_ref()
            .is_none_or(|patch| patch.matches(state))
            && self
                .deletion
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
        Self::capture_targets_with_components(state, scopes, capture_header, specs, true)
    }
    /// Only lexical, audited node-header writers without catalog maintenance.
    pub(crate) fn capture_targets_preserving_components(
        state: &EditorState,
        scopes: Vec<Scope>,
        specs: Vec<TargetSpec>,
    ) -> Result<Self, ScopedHistoryError> {
        let [spec] = specs.as_slice() else {
            return Err(ScopedHistoryError::NodePathChanged);
        };
        if spec.subtree || spec.splice.is_some() || spec.topology.is_some() {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        let target = patch::node(patch::roots(&state.doc, &spec.scope)?, &spec.path)
            .ok_or(ScopedHistoryError::NodePathChanged)?;
        if !state.metadata_target_preserves_components(target) {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        Self::capture_targets_with_components(state, scopes, false, specs, false)
    }
    fn capture_targets_with_components(
        state: &EditorState,
        scopes: Vec<Scope>,
        capture_header: bool,
        specs: Vec<TargetSpec>,
        capture_components: bool,
    ) -> Result<Self, ScopedHistoryError> {
        Ok(Self {
            metadata: ScopedEditorSnapshot::auxiliary_with_components(
                state,
                scopes,
                capture_components,
            ),
            forests: Vec::new(),
            targets: targets::capture(state, specs)?,
            targeted: true,
            clones: None,
            boolean: None,
            deletion: None,
            auxiliary: None,
            header: capture_header.then(|| PendingHeader::capture(&state.doc)),
        })
    }
    pub(crate) fn revision(&self) -> u64 {
        self.metadata.revision
    }
    pub(crate) fn capture(state: &EditorState, scopes: Vec<Scope>, capture_header: bool) -> Self {
        Self::capture_with_components(state, scopes, capture_header, true)
    }
    /// The live atomic driver must upgrade before the first command that is
    /// not proven catalog-preserving, while the original catalog is unchanged.
    pub(crate) fn capture_incremental_preserving_components(state: &EditorState) -> Self {
        Self::capture_with_components(state, Vec::new(), false, false)
    }
    /// Caller has admitted the complete catalog allocation before this upgrade.
    pub(crate) fn upgrade_component_checkpoint(&mut self, state: &EditorState) {
        if !self.metadata.captures_components {
            self.metadata.components = captured_components_checkpoint(state);
            self.metadata.captures_components = true;
        }
    }
    fn capture_with_components(
        state: &EditorState,
        scopes: Vec<Scope>,
        capture_header: bool,
        capture_components: bool,
    ) -> Self {
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
            metadata: ScopedEditorSnapshot::auxiliary_with_components(
                state,
                scopes,
                capture_components,
            ),
            forests,
            targets: Vec::new(),
            targeted: false,
            clones: None,
            boolean: None,
            deletion: None,
            auxiliary: None,
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
        self.prepare_upsert_compaction(state)?;
        self.metadata.validate_shape(state)?;
        for forest in self.forests {
            let (scope, old) = match forest {
                CapturedForest::Root(nodes) => (Scope::Root, nodes),
                CapturedForest::Regions(regions) => {
                    regions.compact(state, &mut self.metadata.patches)?;
                    continue;
                }
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
        if let Some(auxiliary) = self.auxiliary.take() {
            auxiliary.compact_into(state, &mut self.metadata.auxiliary_patches)?;
        }
        self.metadata.header = self.header.map(|header| header.compact(&state.doc));
        let components_unchanged = !self.metadata.captures_components || self.targeted || {
            let current =
                SharedComponents::capture(&state.components, Some(&self.metadata.components));
            Arc::ptr_eq(
                current.components_arc(),
                self.metadata.components.components_arc(),
            ) && current.backing_identity() == self.metadata.components.backing_identity()
        };
        let owns_ui_refs = self.metadata.owns_ui_ref_receipt();
        self.metadata.actor_safe = components_unchanged
            && self
                .metadata
                .auxiliary_patches
                .iter()
                .all(auxiliary::Patch::actor_safe)
            && self.metadata.header.is_none()
            && self.metadata.page_headers.is_empty()
            && self.metadata.app_state_owner == state.app_state_owner
            && (owns_ui_refs
                || (self.metadata.fill_refs == state.ui.variables.fill_refs
                    && self.metadata.stroke_refs == state.ui.variables.stroke_refs))
            && self.metadata.preserve_authored_geometry
                == state.editor_ui.preserve_authored_geometry;
        self.metadata.release_owned_ui_refs();
        self.metadata.patches.shrink_to_fit();
        self.metadata.page_headers.shrink_to_fit();
        Ok(self.metadata)
    }
}
fn captured_components_checkpoint(state: &EditorState) -> SharedComponents {
    SharedComponents::capture(
        &state.components,
        state
            .history
            .future
            .iter()
            .rev()
            .chain(state.history.past.iter().rev())
            .find_map(|entry| match entry {
                HistoryEntry::Full(snapshot) => Some(&snapshot.components),
                HistoryEntry::Scoped(snapshot) if snapshot.captures_components => {
                    Some(snapshot.components())
                }
                HistoryEntry::Scoped(_) => None,
            }),
    )
}
// All non-owning entries share this immutable empty retained-owner marker;
// neither undo nor compaction interprets it as live catalog contents.
fn preserved_components_checkpoint() -> SharedComponents {
    static EMPTY: std::sync::OnceLock<SharedComponents> = std::sync::OnceLock::new();
    EMPTY.get_or_init(SharedComponents::default).clone()
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
        Self::auxiliary_with_components(state, scopes, true)
    }
    fn auxiliary_with_components(
        state: &EditorState,
        scopes: Vec<Scope>,
        captures_components: bool,
    ) -> Self {
        Self {
            history_tag: super::HistoryTag::capture(state.history.actor()),
            actor_safe: false,
            generation: state.document_generation,
            page_count: state.doc.pages.as_ref().map(Vec::len),
            scopes,
            page_patch: None,
            active_page_id: pages::active_id(state),
            patches: Vec::new(),
            auxiliary_patches: Vec::new(),
            page_headers: Vec::new(),
            header: None,
            selection: state.selection.clone(),
            active_page_index: state.ui.active_page_index,
            components: if captures_components {
                captured_components_checkpoint(state)
            } else {
                preserved_components_checkpoint()
            },
            captures_components,
            app_state_owner: state.app_state_owner.clone(),
            fill_refs: state.ui.variables.fill_refs.clone(),
            stroke_refs: state.ui.variables.stroke_refs.clone(),
            preserve_authored_geometry: state.editor_ui.preserve_authored_geometry,
            revision: state.revision,
        }
    }
    pub(crate) fn has_canonical_changes(&self) -> bool {
        self.page_patch.is_some()
            || !self.patches.is_empty()
            || !self.auxiliary_patches.is_empty()
            || !self.page_headers.is_empty()
            || self.header.is_some()
    }
    pub(crate) fn scopes(&self) -> Vec<Scope> {
        self.scopes.clone()
    }
    pub(crate) fn captures_header(&self) -> bool {
        self.header.is_some()
            || self
                .auxiliary_patches
                .iter()
                .any(auxiliary::Patch::is_header)
    }
    /// Retained catalog checkpoint; empty when this entry deliberately leaves
    /// catalog state untouched. Default snapshots and arbitrary-maintenance
    /// constructors retain complete catalog ownership.
    pub fn components(&self) -> &SharedComponents {
        &self.components
    }
    pub fn captures_components(&self) -> bool {
        self.captures_components
    }
    pub fn retained_roots(&self) -> impl Iterator<Item = &Arc<SharedNode>> {
        self.page_patch
            .iter()
            .flat_map(pages::Patch::retained_roots)
            .chain(self.patches.iter().flat_map(Patch::retained_roots))
            .chain(
                self.auxiliary_patches
                    .iter()
                    .flat_map(auxiliary::Patch::retained_roots),
            )
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
        let mut total = self
            .page_patch
            .as_ref()
            .map_or(0, pages::Patch::storage_bytes)
            + self.active_page_id.as_ref().map_or(0, String::capacity)
            + mem::size_of::<Self>()
            + 2 * mem::size_of::<usize>()
            + self.scopes.capacity() * mem::size_of::<Scope>()
            + self.patches.capacity() * mem::size_of::<Patch>()
            + self.auxiliary_patches.capacity() * mem::size_of::<auxiliary::Patch>()
            + self
                .auxiliary_patches
                .iter()
                .map(auxiliary::Patch::storage_bytes)
                .sum::<usize>()
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
            if let Scope::Page { index, id } = scope {
                if !state
                    .doc
                    .pages
                    .as_ref()
                    .and_then(|pages| pages.get(*index))
                    .is_some_and(|page| page.id == *id)
                {
                    return Err(ScopedHistoryError::PageIdentityChanged { index: *index });
                }
            }
            patch::roots(&state.doc, scope)?;
        }
        Ok(())
    }
    /// All destinations are validated before any write. One historical side
    /// becomes the reverse entry; no permanent before/after document pair.
    pub(crate) fn exchange(&mut self, state: &mut EditorState) -> Result<(), ScopedHistoryError> {
        self.validate_shape(state)?;
        let catalog = splice_exchange::Catalog::new(&state.history, &self.patches, &state.doc);
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
        let auxiliary = auxiliary::Prepared::capture(state, &self.auxiliary_patches, false)?;
        let page = self
            .page_patch
            .as_ref()
            .map(|patch| pages::Prepared::capture(state, patch))
            .transpose()?;
        let active = pages::Active::capture(state);
        let reverse =
            Self::auxiliary_with_components(state, self.scopes.clone(), self.captures_components);
        let mut reordered = patch::Reorders::default();
        if let Some(splices) = splices {
            splices.exchange(&mut state.doc, &mut self.patches);
            for patch in &mut self.patches {
                if patch.splices().next().is_none() {
                    patch.exchange(&mut state.doc, &mut reordered);
                }
            }
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
        if let Some(page) = page {
            page.exchange(state, self.page_patch.as_mut().unwrap());
        }
        auxiliary.exchange(state, &mut self.auxiliary_patches);
        state.selection = mem::replace(&mut self.selection, reverse.selection);
        pages::exchange_active(self, state, active);
        if self.captures_components {
            state.components = self.components.materialize();
            self.components = reverse.components;
        }
        state.app_state_owner = mem::replace(&mut self.app_state_owner, reverse.app_state_owner);
        self.exchange_legacy_ui_refs(state, reverse.fill_refs, reverse.stroke_refs);
        state.editor_ui.preserve_authored_geometry = mem::replace(
            &mut self.preserve_authored_geometry,
            reverse.preserve_authored_geometry,
        );
        state.revision = mem::replace(&mut self.revision, reverse.revision);
        state.sync_dirty_flag();
        Ok(())
    }
}
