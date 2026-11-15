//! Token census shares the lexical provenance of CommandIdSequence, not a
//! revision-only public-document cache. Unknown writes discard it before apply.
use super::{EditorState, SeedOverride, C};
use crate::variables_resolve::reference_index::{Cache, Index, Route};
use std::sync::Arc;
fn preserves(command: &C) -> bool {
    if !command.may_change_document() {
        return true;
    }
    match command {
        C::RenameVariable { .. }
        | C::DeleteVariable { .. }
        | C::CreateVariable { .. }
        | C::SetVariableColor { .. }
        | C::SetVariableScalar { .. }
        | C::SetVariables { .. }
        | C::UpsertVariables { .. }
        | C::SetThemes { .. }
        | C::MergeThemePreset { .. }
        | C::SetDesignMd { .. }
        | C::SetNodeName { .. }
        | C::SetNodeFlag { .. }
        | C::SetNodeFlip { .. }
        | C::SetNodeRotation { .. }
        | C::SetNodeFontSize { .. }
        | C::SetNodeFontWeight { .. } => true,
        C::Batch { commands } => commands
            .iter()
            .all(|c| !matches!(c, C::Batch { .. }) && preserves(c)),
        _ => false,
    }
}
impl SeedOverride {
    pub(super) fn observe_references(&self, command: &C) {
        if !preserves(command) {
            if let Ok(mut cache) = self.references.lock() {
                *cache = Cache::Empty;
            }
        }
    }
    pub(super) fn take_reference_index(&mut self) -> Option<Index> {
        std::mem::take(&mut self.references)
            .into_inner()
            .ok()?
            .finish()
    }
}
impl EditorState {
    pub(crate) fn trusted_variable_reference_routes(&self, name: &str) -> Option<Vec<Arc<Route>>> {
        let seed = &self.command_id_seed;
        if !seed.entered || !seed.valid || seed.generation != self.document_generation() {
            return None;
        }
        seed.references.lock().ok()?.routes(&self.doc, name)
    }
    /// A fresh complete census is used outside an observed authority; public Doc
    /// mutation never inherits another request's route or negative-reference proof.
    pub(crate) fn variable_reference_routes(&self, name: &str) -> Option<Vec<Arc<Route>>> {
        let seed = &self.command_id_seed;
        if seed.entered && seed.valid && seed.generation == self.document_generation() {
            self.trusted_variable_reference_routes(name)
        } else {
            Index::build(&self.doc)?.routes(name)
        }
    }
    pub(crate) fn clear_variable_reference_index(&self) {
        if let Ok(mut cache) = self.command_id_seed.references.lock() {
            *cache = Cache::Empty;
        }
    }
    pub(crate) fn refresh_variable_reference_index(&self, routes: &[Arc<Route>]) {
        let seed = &self.command_id_seed;
        if seed.entered && seed.valid && seed.generation == self.document_generation() {
            if let Ok(mut cache) = seed.references.lock() {
                cache.refresh(&self.doc, routes);
            }
        }
    }
}
