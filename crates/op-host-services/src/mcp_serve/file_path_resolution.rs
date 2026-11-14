//! Only one resolution per distinct pathname within a routed request.
//! The entry never survives the request: external replacement/stamp/recovery
//! checks remain at the authority boundary, and each next call resolves anew.
use super::super::resident_documents::{AuthorityError, CanonicalPath};
use std::path::{Path, PathBuf};

#[derive(Default)]
pub(super) struct RequestPaths {
    entries: Vec<(PathBuf, Result<CanonicalPath, AuthorityError>)>,
}
impl RequestPaths {
    pub(super) fn after_coordination_wait(&mut self) {
        // A symlink or missing ancestor may have changed while a copy waited
        // behind another writer. Establish new resolutions inside that lock.
        self.entries.clear();
    }
    pub(super) fn resolve(&mut self, path: &Path) -> Result<CanonicalPath, AuthorityError> {
        if let Some((_, result)) = self.entries.iter().find(|(key, _)| key == path) {
            return result.clone();
        }
        let result = CanonicalPath::resolve(path);
        self.entries.push((path.to_path_buf(), result.clone()));
        result
    }
    pub(super) fn same(&mut self, a: &Path, b: &Path) -> bool {
        match (self.resolve(a), self.resolve(b)) {
            (Ok(a), Ok(b)) => a.as_path() == b.as_path(),
            _ => false,
        }
    }
    pub(super) fn is_current(&mut self, target: &CanonicalPath, current: Option<&Path>) -> bool {
        current.is_some_and(|current| {
            self.resolve(current)
                .is_ok_and(|p| p.as_path() == target.as_path())
        })
    }
}
