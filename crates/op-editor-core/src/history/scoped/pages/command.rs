//! One intended page edit and exact catalog metadata, prepared while the full
//! real document is exclusively held. No standalone page table is substituted.
use super::super::{
    auxiliary::{AddedPage, PreparedPageCatalog},
    splice_order::Anchors,
    ScopedEditorSnapshot,
};
use super::{collision, exchange, forecast::Forecast, routes, shared, Kind, Migration, Patch};
use crate::{
    page_mutators::{CommittedPageCommand, PreparedPageCommand},
    EditorCommand, EditorState,
};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
};

pub(crate) struct PreparedEdit {
    plan: PreparedPageCommand,
    snapshot: ScopedEditorSnapshot,
    catalogue: PreparedPageCatalog,
    routes: Vec<routes::Entry>,
    before: Vec<String>,
    external: HashMap<String, usize>,
    references: Arc<Vec<String>>,
    next: u64,
}
fn anchor(ids: &[String], index: usize, id: &str) -> Anchors {
    Anchors {
        ids: vec![id.to_owned()],
        left: index.checked_sub(1).and_then(|at| ids.get(at)).cloned(),
        right: ids.get(index + 1).cloned(),
        required: false,
    }
}
fn references(roots: &[jian_ops_schema::node::PenNode]) -> Arc<Vec<String>> {
    use crate::PenNodeExt;
    let mut ids = BTreeSet::new();
    let mut pending = vec![roots.iter()];
    while let Some(nodes) = pending.last_mut() {
        if let Some(node) = nodes.next() {
            if let jian_ops_schema::node::PenNode::Ref(reference) = node {
                ids.insert(reference.target.clone());
                let mut values = reference
                    .descendants
                    .iter()
                    .flat_map(|map| map.values())
                    .collect::<Vec<_>>();
                while let Some(value) = values.pop() {
                    match value {
                        serde_json::Value::Object(map) => {
                            if let Some(target) = map.get("ref").and_then(serde_json::Value::as_str)
                            {
                                ids.insert(target.to_owned());
                            }
                            values.extend(map.values());
                        }
                        serde_json::Value::Array(array) => values.extend(array),
                        _ => {}
                    }
                }
            }
            if let Some(children) = node.children() {
                pending.push(children.iter());
            }
        } else {
            pending.pop();
        }
    }
    Arc::new(ids.into_iter().collect())
}
impl PreparedEdit {
    pub(super) fn prepare(
        state: &EditorState,
        command: EditorCommand,
        bound: Forecast,
    ) -> Option<Self> {
        let ids_needed = matches!(
            command,
            EditorCommand::AddPage { .. } | EditorCommand::DuplicatePage { .. }
        );
        let allocator = if ids_needed {
            state.standalone_id_allocator().ok()?
        } else {
            crate::DocumentIdAllocator::sequential(1)
        };
        let mut allocator = crate::command_apply_legacy::LeafHighWaterAllocator(allocator);
        let plan = PreparedPageCommand::new(state, command, &mut allocator).ok()??;
        let added = plan.added().map(|page| AddedPage {
            page,
            index_hint: plan.added_index().unwrap(),
            source: plan.source(state),
        });
        let removed = plan.removed(state);
        if PreparedPageCatalog::forecast(state, added, removed)? > bound.catalogue {
            return None;
        }
        let catalogue = PreparedPageCatalog::prepare(state, added, removed)?;
        let references = references(
            added
                .map(|page| page.page.children.as_slice())
                .or_else(|| removed.map(|page| page.children.as_slice()))
                .unwrap_or(&[]),
        );
        let removing = removed
            .map(|page| [page.id.clone()].into_iter().collect::<HashSet<_>>())
            .unwrap_or_default();
        let external = collision::for_removed(state, &removing);
        let routes = routes::prepare(state, plan.promoted_id());
        let before = exchange::ids(state).ok()?;
        Some(Self {
            plan,
            catalogue,
            routes,
            before,
            external,
            references,
            snapshot: ScopedEditorSnapshot::auxiliary_with_components(state, Vec::new(), false),
            next: allocator.0.next_counter(),
        })
    }
    pub(crate) fn commit(mut self, state: &mut EditorState) -> Arc<ScopedEditorSnapshot> {
        let changed = self.plan.commit(state);
        routes::install(state, &self.routes);
        let (kind, placement, affected, migration) = match changed {
            CommittedPageCommand::Insert { id, at, legacy } => {
                let after = exchange::ids(state).expect("prepared fresh page identities");
                let migration = legacy.map(|legacy_present| {
                    let promoted = &state.doc.pages.as_ref().unwrap()[0];
                    Migration {
                        page: promoted.id.clone(),
                        header: shared::header(promoted),
                        legacy_present,
                        promoted: true,
                        routes: self.routes,
                    }
                });
                let mut affected = vec![id.clone()];
                if let Some(migration) = &migration {
                    affected.push(migration.page.clone());
                }
                (
                    Kind::Splice {
                        expected: vec![id.clone()],
                        old: Vec::new(),
                    },
                    anchor(&after, at, &id),
                    affected,
                    migration,
                )
            }
            CommittedPageCommand::Delete { page, index } => {
                let id = page.id.clone();
                (
                    Kind::Splice {
                        expected: Vec::new(),
                        old: vec![shared::SharedPage::owned(page)],
                    },
                    anchor(&self.before, index, &id),
                    vec![id],
                    None,
                )
            }
            CommittedPageCommand::Move { id, from } => (
                Kind::Move { id: id.clone() },
                anchor(&self.before, from, &id),
                vec![id],
                None,
            ),
        };
        self.catalogue.commit(state, &mut self.snapshot);
        let mut components = self
            .snapshot
            .auxiliary_patches
            .iter()
            .filter_map(|patch| {
                if let super::super::auxiliary::Patch::Catalog(entry) = patch {
                    Some(entry.id.as_str().to_owned())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        components.sort_unstable();
        components.dedup();
        self.snapshot.page_patch = Some(Patch {
            kind,
            placement,
            affected: Arc::new(affected),
            migration,
            recent: Vec::new(),
            external: self.external,
            component_ids: Arc::new(components),
            references: self.references,
        });
        self.snapshot.page_count = state.doc.pages.as_ref().map(Vec::len);
        self.snapshot.actor_safe = true;
        state.record_runtime_next_id(self.next);
        state.mark_document_changed();
        Arc::new(self.snapshot)
    }
}
impl ScopedEditorSnapshot {
    pub(crate) fn page_command_forecast(
        state: &EditorState,
        command: &EditorCommand,
        limit: usize,
    ) -> Option<Forecast> {
        if state.undo_capture_suspended
            || state.editor_ui.workspace.active
            || state.instance_write_virtual_anchor.is_some()
        {
            return None;
        }
        super::forecast::command(state, command, limit)
    }
    pub(crate) fn prepare_page_command(
        state: &EditorState,
        command: EditorCommand,
        bound: Forecast,
    ) -> Option<PreparedEdit> {
        PreparedEdit::prepare(state, command, bound)
    }
}

impl EditorState {
    pub(crate) fn prepare_page_catalog_change(
        &self,
        added: Option<(
            &jian_ops_schema::page::PenPage,
            usize,
            Option<&[jian_ops_schema::node::PenNode]>,
        )>,
        removed: Option<&jian_ops_schema::page::PenPage>,
    ) -> Option<PreparedPageCatalog> {
        let added = added.map(|(page, index_hint, source)| AddedPage {
            page,
            index_hint,
            source,
        });
        PreparedPageCatalog::prepare(self, added, removed)
    }
    pub(crate) fn prepare_page_root_routes(
        &self,
        promoted: Option<&str>,
    ) -> super::routes::Transition {
        super::routes::Transition::prepare(self, promoted)
    }
}
