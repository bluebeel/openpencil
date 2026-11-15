//! Exact, authority-owned incremental compact saves. Mutable access invalidates
//! the cache; only completed metadata receipts may advance an extracted chain.
use super::DocIoError;
use crate::mcp_serve::file_ownership::FileVersion;
use jian_ops_schema::{image_thumbs::ImageThumbSnapshot, node::PenNode};
use op_editor_core::{EditorState, PenNodeExt};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
};
mod budget;
mod index;
mod splice;
mod tracked;
use index::{Index, Span};
pub(crate) use tracked::TrackedEditor;

/// Writer-derived byte cache, never a second canonical document. A caller may
/// retain this only behind TrackedEditor and while all mutations are observed.
#[derive(Default)]
pub(crate) struct SaveChain {
    base: Option<Base>,
}
struct Base {
    raw: Vec<u8>,
    index: Index,
    replacements: BTreeMap<Span, Vec<u8>>,
    replacement_bytes: usize,
    path: PathBuf,
    stamp: FileVersion,
    generation: u64,
    revision: u64,
    thumbnails: ImageThumbSnapshot,
    _reservation: budget::Reservation,
}
impl SaveChain {
    pub(crate) fn invalidate(&mut self) {
        self.base = None;
    }
    pub(crate) fn matches(&self, state: &EditorState) -> bool {
        self.base.as_ref().is_some_and(|base| {
            base.generation == state.document_generation()
                && base.revision == state.document_revision()
        })
    }
    /// Call immediately after each completed command, before another command.
    /// A false proof, changed IDs/resources, partial visit or bounds exhaustion
    /// discards the whole derived cache; the canonical writer remains fallback.
    pub(crate) fn observe(
        &mut self,
        state: &EditorState,
        visit: impl FnOnce(&mut dyn FnMut(&PenNode, &PenNode)) -> bool,
    ) {
        let Some(base) = self.base.as_mut() else {
            return;
        };
        let mut valid = base.generation == state.document_generation();
        let exact = visit(&mut |old, current| {
            if !valid {
                return;
            }
            valid = old.id_str() == current.id_str() && base.patch(old, current).is_some();
        });
        if !exact || !valid {
            self.invalidate();
        }
    }
    pub(crate) fn save(&mut self, state: &EditorState, path: &Path) -> Result<(), DocIoError> {
        let thumbs = jian_ops_schema::image_thumbs::capture_snapshot();
        if let Some(base) = &mut self.base {
            if base.generation == state.document_generation()
                && base.thumbnails.same_contents(&thumbs)
            {
                if let Some(meta) = splice::encode(&op_pen_loader::EditorMeta::from_state(state)) {
                    let span = base.index.meta;
                    if base.replace(span, meta).is_some() {
                        // A real I/O/ownership failure is returned, never hidden
                        // by a second save attempt or an acknowledgement.
                        match base.write(path) {
                            Ok(true) => {
                                base.revision = state.document_revision();
                                return Ok(());
                            }
                            Ok(false) => (),
                            Err(error) => {
                                self.base = None;
                                return Err(error);
                            }
                        }
                    }
                }
            }
        }
        self.base = None;
        let mut seed = Seed::new();
        super::save_compact_recorded(state, path, seed.as_mut(), &thumbs)?;
        if let Some(seed) = seed {
            if let (Some(path), Some(stamp)) = (seed.path, seed.stamp) {
                if let Some(index) = Index::new(&seed.raw) {
                    let mut reservation = seed.reservation;
                    let retained = seed.raw.capacity()
                        + index.bytes()
                        + budget::EDITS
                        + thumbs.storage_bytes();
                    if retained > reservation.0 {
                        return Ok(());
                    }
                    reservation.shrink(retained);
                    self.base = Some(Base {
                        raw: seed.raw,
                        index,
                        replacements: BTreeMap::new(),
                        replacement_bytes: 0,
                        path,
                        stamp,
                        generation: state.document_generation(),
                        revision: state.document_revision(),
                        thumbnails: thumbs,
                        _reservation: reservation,
                    });
                }
            }
        }
        Ok(())
    }
}
impl Base {
    fn replace(&mut self, span: Span, replacement: Vec<u8>) -> Option<()> {
        let old = self.replacements.get(&span).map_or(0, Vec::capacity);
        let bytes = self
            .replacement_bytes
            .checked_sub(old)?
            .checked_add(replacement.capacity())?;
        if (!self.replacements.contains_key(&span) && self.replacements.len() >= 8192)
            || bytes > budget::EDITS - 1024 * 1024
        {
            return None;
        }
        use std::ops::Bound::{Excluded, Unbounded};
        for (other, _) in self
            .replacements
            .range(..span)
            .next_back()
            .into_iter()
            .chain(self.replacements.range((Excluded(span), Unbounded)).next())
        {
            if span.start < other.end && other.start < span.end {
                return None;
            }
        }
        self.replacement_bytes = bytes;
        self.replacements.insert(span, replacement);
        Some(())
    }
    fn current(&self, span: Span) -> &[u8] {
        self.replacements
            .get(&span)
            .map_or(&self.raw[span.start..span.end], Vec::as_slice)
    }
    fn patch(&mut self, old: &PenNode, current: &PenNode) -> Option<()> {
        let node = self.index.nodes.get(old.id_str()).copied().flatten()?;
        let before = splice::metadata(old)?;
        let after = splice::metadata(current)?;
        let old_parts = splice::parts(&before)?;
        let new_parts = splice::parts(&after)?;
        match (node.children, old_parts, new_parts) {
            (None, None, None) => {
                if self.current(node.object) != before.as_slice() {
                    return None;
                }
                self.replace(node.object, after)
            }
            (Some(children), Some(a), Some(b)) => {
                let prefix = Span {
                    start: node.object.start,
                    end: children.start,
                };
                let suffix = Span {
                    start: children.end,
                    end: node.object.end,
                };
                if self.current(prefix) != &before[..a.start]
                    || self.current(suffix) != &before[a.end..]
                {
                    return None;
                }
                self.replace(prefix, after[..b.start].to_vec())?;
                self.replace(suffix, after[b.end..].to_vec())
            }
            _ => None,
        }
    }
    fn write(&mut self, requested: &Path) -> Result<bool, DocIoError> {
        let authorization =
            crate::mcp_serve::file_ownership::authorize_save(requested).map_err(|e| {
                DocIoError::FileOwnership {
                    detail: e.to_string(),
                    committed: false,
                }
            })?;
        let _scope = authorization.scope();
        let path = authorization.path();
        let stamp = std::fs::metadata(path)
            .map(|m| FileVersion::metadata(&m))
            .map_err(|e| DocIoError::Io(e.to_string()))?;
        if path != self.path || stamp != self.stamp {
            return Ok(false);
        }
        let (temp, file) = super::create_sibling_temp(path)?;
        let result = (|| {
            super::save_buffer::with_shared_buffered_file(file, |writer| {
                let mut at = 0;
                for (span, bytes) in &self.replacements {
                    writer
                        .write_all(&self.raw[at..span.start])
                        .map_err(|e| DocIoError::Io(e.to_string()))?;
                    writer
                        .write_all(bytes)
                        .map_err(|e| DocIoError::Io(e.to_string()))?;
                    at = span.end;
                }
                writer
                    .write_all(&self.raw[at..])
                    .map_err(|e| DocIoError::Io(e.to_string()))
            })?;
            let next = std::fs::metadata(&temp)
                .map(|m| FileVersion::metadata(&m))
                .map_err(|e| DocIoError::Io(e.to_string()))?;
            super::commit_staged_document(&temp, path)?;
            self.stamp = next;
            Ok(true)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result
    }
}
/// Optional bounded tee populated by the real canonical writer itself. Reading
/// a later destination file cannot establish that its bytes match the editor.
pub(super) struct Seed {
    raw: Vec<u8>,
    reservation: budget::Reservation,
    path: Option<PathBuf>,
    stamp: Option<FileVersion>,
}
impl Seed {
    fn new() -> Option<Self> {
        Some(Self {
            raw: Vec::new(),
            reservation: budget::Reservation::new()?,
            path: None,
            stamp: None,
        })
    }
    pub(super) fn bind(&mut self, path: &Path) {
        self.path = Some(path.to_path_buf());
    }
    pub(super) fn staged(&mut self, path: &Path) {
        self.stamp = std::fs::metadata(path)
            .ok()
            .map(|m| FileVersion::metadata(&m));
    }
    pub(super) fn writer<'a, W: Write>(&'a mut self, writer: &'a mut W) -> SeedWriter<'a, W> {
        SeedWriter { writer, seed: self }
    }
}
pub(super) struct SeedWriter<'a, W> {
    writer: &'a mut W,
    seed: &'a mut Seed,
}
impl<W: Write> Write for SeedWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.writer.write_all(bytes)?;
        if self.seed.path.is_some() {
            if let Some(next) = self
                .seed
                .raw
                .len()
                .checked_add(bytes.len())
                .filter(|n| *n <= budget::RAW)
            {
                let capacity = self
                    .seed
                    .raw
                    .capacity()
                    .saturating_mul(2)
                    .max(next)
                    .min(budget::RAW);
                if next > self.seed.raw.capacity()
                    && self
                        .seed
                        .raw
                        .try_reserve_exact(capacity - self.seed.raw.len())
                        .is_err()
                {
                    self.seed.path = None;
                    self.seed.raw = Vec::new();
                } else {
                    self.seed.raw.extend_from_slice(bytes);
                }
            } else {
                self.seed.path = None;
                self.seed.raw = Vec::new();
            }
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}
