//! Validate the complete page-table exchange before moving any live page.
use super::super::splice_order::{self, Anchors, Token};
use super::{shared, Kind, Patch, ScopedHistoryError};
use crate::{EditorState, PenNodeExt};
use jian_ops_schema::page::PenPage;
use std::{
    collections::{HashMap, HashSet},
    mem,
};

pub(in crate::history::scoped) struct Prepared {
    order: Vec<Token>,
    removing: HashSet<String>,
    restored: Vec<PenPage>,
    inverse: Anchors,
    external: HashMap<String, usize>,
}
pub(super) fn ids(state: &EditorState) -> Result<Vec<String>, ScopedHistoryError> {
    let mut unique = HashSet::new();
    state
        .doc
        .pages
        .iter()
        .flatten()
        .map(|page| {
            if !unique.insert(page.id.as_str()) {
                return Err(ScopedHistoryError::PageShapeChanged);
            }
            Ok(page.id.clone())
        })
        .collect()
}
fn anchor(ids: &[String], owned: &[String]) -> Result<Anchors, ScopedHistoryError> {
    let positions: Vec<_> = owned
        .iter()
        .map(|id| {
            ids.iter()
                .position(|current| current == id)
                .ok_or(ScopedHistoryError::PageShapeChanged)
        })
        .collect::<Result<_, _>>()?;
    if positions.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(ScopedHistoryError::PageShapeChanged);
    }
    let first = positions.first().copied().unwrap_or(ids.len());
    let last = positions.last().map_or(first, |last| last + 1);
    Ok(Anchors {
        ids: owned.to_vec(),
        left: first.checked_sub(1).map(|at| ids[at].clone()),
        right: ids.get(last).cloned(),
        required: false,
    })
}
impl Prepared {
    pub(in crate::history::scoped) fn capture(
        state: &EditorState,
        patch: &Patch,
    ) -> Result<Self, ScopedHistoryError> {
        if let Some(migration) = &patch.migration {
            super::routes::validate(state, &migration.routes)?;
        }
        let current = ids(state)?;
        let removing: HashSet<String> = match &patch.kind {
            Kind::Splice { expected, .. } => expected.iter().cloned().collect(),
            Kind::Move { id } => [id.clone()].into_iter().collect(),
        };
        if !removing.iter().all(|id| current.contains(id)) {
            return Err(ScopedHistoryError::PageShapeChanged);
        }
        let expected = current
            .iter()
            .filter(|id| removing.contains(*id))
            .cloned()
            .collect::<Vec<_>>();
        if let Kind::Splice {
            expected: authored, ..
        } = &patch.kind
        {
            if *authored != expected {
                return Err(ScopedHistoryError::PageShapeChanged);
            }
        }
        let inverse = anchor(&current, &expected)?;
        let mut remaining = current
            .iter()
            .filter(|id| !removing.contains(*id))
            .cloned()
            .collect::<Vec<_>>();
        if let Some(migration) = &patch.migration {
            if migration.promoted {
                if !state.doc.children.is_empty() || remaining != [migration.page.clone()] {
                    return Err(ScopedHistoryError::PageShapeChanged);
                }
                let page = state
                    .doc
                    .pages
                    .as_ref()
                    .unwrap()
                    .iter()
                    .find(|page| page.id == migration.page)
                    .unwrap();
                if !super::super::header_map::exact(&shared::header(page), &migration.header) {
                    return Err(ScopedHistoryError::DocumentHeaderChanged);
                }
            } else {
                if !current.is_empty() || state.doc.pages.is_some() != migration.legacy_present {
                    return Err(ScopedHistoryError::PageShapeChanged);
                }
                remaining.push(migration.page.clone());
            }
        } else if state.doc.pages.as_ref().is_none_or(Vec::is_empty) {
            return Err(ScopedHistoryError::PageShapeChanged);
        }
        let restoring = match &patch.kind {
            Kind::Splice { old, .. } => old
                .iter()
                .map(|page| page.id().to_owned())
                .collect::<Vec<_>>(),
            Kind::Move { id } => vec![id.clone()],
        };
        let mut witnesses = Vec::new();
        for snapshot in state
            .history
            .past
            .iter()
            .chain(&state.history.future)
            .filter_map(|entry| entry.scoped_snapshot())
        {
            if let Some(other) = &snapshot.page_patch {
                // An earlier placement of the same owned page is not a second
                // constraint on a move. Foreign page witnesses stay intact.
                if !other
                    .placement
                    .ids
                    .iter()
                    .any(|id| removing.contains(id) || restoring.contains(id))
                {
                    witnesses.push(other.placement.clone());
                    witnesses.extend(other.recent.iter().cloned());
                }
            }
        }
        let mut required = patch.placement.clone();
        required.required = !restoring.is_empty();
        witnesses.push(required);
        witnesses.extend(patch.recent.iter().cloned());
        let order = if patch
            .migration
            .as_ref()
            .is_some_and(|migration| migration.promoted)
        {
            Vec::new()
        } else {
            splice_order::order(&remaining, &restoring, witnesses.into_iter())?
        };
        if !super::collision::validate(state, patch, &removing) {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        let external = if matches!(patch.kind, Kind::Splice { .. }) {
            super::collision::for_removed(state, &removing)
        } else {
            HashMap::new()
        };
        // Materialize only the restoring pages, after all order validation.
        // Live pages and their descendants stay owned and are moved at commit.
        let restored = match &patch.kind {
            Kind::Splice { old, .. } => old.iter().map(|page| page.materialize()).collect(),
            Kind::Move { .. } => Vec::new(),
        };
        Ok(Self {
            order,
            removing,
            restored,
            inverse,
            external,
        })
    }
    pub(in crate::history::scoped) fn exchange(self, state: &mut EditorState, patch: &mut Patch) {
        let mut pages = state.doc.pages.take().unwrap_or_default();
        let mut removed = HashMap::new();
        let mut remaining = Vec::with_capacity(pages.len());
        for page in pages.drain(..) {
            if self.removing.contains(&page.id) {
                removed.insert(page.id.clone(), page);
            } else {
                remaining.push(page);
            }
        }
        if let Some(migration) = &mut patch.migration {
            if migration.promoted {
                let mut page = remaining.pop().expect("validated sole promoted page");
                state.doc.children = mem::take(&mut page.children);
                state.doc.pages = migration.legacy_present.then(Vec::new);
            } else {
                let mut page = migration.header.clone();
                page.children = mem::take(&mut state.doc.children);
                remaining.push(page);
            }
            migration.promoted = !migration.promoted;
        }
        let mut restore = match &patch.kind {
            Kind::Splice { .. } => self.restored,
            Kind::Move { id } => vec![removed.remove(id).expect("validated moved page")],
        };
        if !patch
            .migration
            .as_ref()
            .is_some_and(|migration| !migration.promoted)
        {
            let mut existing = remaining.into_iter().map(Some).collect::<Vec<_>>();
            let mut restoring = restore.drain(..).map(Some).collect::<Vec<_>>();
            let reordered = self
                .order
                .into_iter()
                .map(|token| match token {
                    Token::Existing(at) => existing[at].take().unwrap(),
                    Token::Restore(at) => restoring[at].take().unwrap(),
                })
                .collect();
            state.doc.pages = Some(reordered);
        }
        if let Kind::Splice { expected, old } = &mut patch.kind {
            let inverse = expected
                .iter()
                .map(|id| shared::SharedPage::owned(removed.remove(id).unwrap()))
                .collect();
            *expected = old.iter().map(|page| page.id().to_owned()).collect();
            *old = inverse;
        }
        if let Some(migration) = &mut patch.migration {
            super::routes::exchange(state, &mut migration.routes);
        }
        patch.placement = self.inverse;
        patch.external = self.external;
        patch.recent.clear();
    }
}
