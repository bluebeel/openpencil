//! Grow one exact before-image while exclusively executing classified commands.
//! Narrow physical edits retain each original top-level root at most once.
use super::*;

impl PendingScopedSnapshot {
    pub(crate) fn extend_before_change(
        &mut self,
        state: &EditorState,
        scopes: Vec<Scope>,
        header: bool,
        targets: Option<&[TargetSpec]>,
    ) -> Result<(), ScopedHistoryError> {
        self.metadata.validate_shape(state)?;
        // Strict plans reject id/type/children metadata, virtual Ref ancestry,
        // and ambiguous splice routes. Topology/subtree rewrites upgrade to a
        // complete original forest before the command is allowed to run.
        let narrow = targets.filter(|specs| {
            specs
                .iter()
                .all(|spec| !spec.subtree && spec.topology.is_none())
        });
        for scope in scopes {
            let existing = self
                .metadata
                .scopes
                .iter()
                .position(|captured| captured == &scope);
            if let Some(index) = existing {
                if let CapturedForest::Regions(regions) = &mut self.forests[index] {
                    if let Some(targets) = narrow {
                        regions.capture_targets(state, targets)?;
                    } else {
                        regions.validate(&state.doc)?;
                        // Validation above precedes removal; into_full cannot
                        // lose an inverse to a missing/ambiguous live source.
                        let CapturedForest::Regions(regions) = self.forests.remove(index) else {
                            unreachable!()
                        };
                        let full = regions.into_full(state)?;
                        self.forests.insert(index, full);
                    }
                }
                continue;
            }
            let captured = if let Some(targets) = narrow {
                match incremental_regions::PendingRegions::capture(state, scope.clone())? {
                    Some(mut regions) => {
                        regions.capture_targets(state, targets)?;
                        CapturedForest::Regions(regions)
                    }
                    None => full_forest(state, &scope)?,
                }
            } else {
                full_forest(state, &scope)?
            };
            self.forests.push(captured);
            self.metadata.scopes.push(scope);
        }
        if header && self.header.is_none() {
            self.header = Some(PendingHeader::capture(&state.doc));
        }
        Ok(())
    }
    pub(crate) fn validate_incremental(
        &self,
        state: &EditorState,
    ) -> Result<(), ScopedHistoryError> {
        self.metadata.validate_shape(state)?;
        for forest in &self.forests {
            if let CapturedForest::Regions(regions) = forest {
                regions.validate(&state.doc)?;
            }
        }
        for scope in &self.metadata.scopes {
            patch::roots(&state.doc, scope)?;
        }
        Ok(())
    }
}
fn full_forest(state: &EditorState, scope: &Scope) -> Result<CapturedForest, ScopedHistoryError> {
    Ok(match scope {
        Scope::Root => CapturedForest::Root(state.doc.children.clone()),
        Scope::Page { index, id } => {
            let page = state
                .doc
                .pages
                .as_ref()
                .and_then(|pages| pages.get(*index))
                .ok_or(ScopedHistoryError::PageMissing { index: *index })?;
            if page.id != *id {
                return Err(ScopedHistoryError::PageIdentityChanged { index: *index });
            }
            CapturedForest::Page {
                index: *index,
                page: page.clone(),
            }
        }
    })
}
