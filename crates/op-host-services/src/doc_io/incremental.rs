//! Exact, authority-owned incremental compact saves. Completed staging files
//! back unchanged ranges; only completed metadata receipts advance a chain.
use super::DocIoError;
use crate::mcp_serve::file_ownership::FileVersion;
use jian_ops_schema::{image_thumbs::ImageThumbSnapshot, node::PenNode};
use op_editor_core::{EditorState, PenNodeExt};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
mod budget;
mod index;
mod splice;
mod stats;
mod tracked;
use index::{Index, Span};
pub(crate) use stats::snapshot as snapshot_stats;
use stats::{hit, Event};
pub(crate) use tracked::TrackedEditor;

/// Writer-derived byte cache, never a second canonical document. A caller may
/// retain this only behind TrackedEditor and while all mutations are observed.
#[derive(Default)]
pub(crate) struct SaveChain {
    base: Option<Base>,
}
struct Base {
    raw: File,
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
            if !valid {
                hit(Event::MetadataMismatch);
            }
        });
        if !exact || !valid {
            if !exact {
                hit(Event::ReceiptRefusal);
            }
            self.invalidate();
        }
    }
    pub(crate) fn save(&mut self, state: &EditorState, path: &Path) -> Result<(), DocIoError> {
        let thumbs = jian_ops_schema::image_thumbs::capture_snapshot();
        if let Some(base) = &mut self.base {
            if base.generation == state.document_generation()
                && base.thumbnails.same_contents(&thumbs)
            {
                if let Some(meta) =
                    splice::editor_meta(&op_pen_loader::EditorMeta::from_state(state))
                {
                    let span = base.index.meta;
                    if base.replace(span, meta).is_some() {
                        // A real I/O/ownership failure is returned, never hidden
                        // by a second save attempt or an acknowledgement.
                        match base.write(path) {
                            Ok(true) => {
                                base.revision = state.document_revision();
                                hit(Event::IncrementalSave);
                                return Ok(());
                            }
                            Ok(false) => hit(Event::SourceDrift),
                            Err(error) => {
                                self.base = None;
                                return Err(error);
                            }
                        }
                    }
                } else {
                    hit(Event::MetaLimit);
                }
            } else {
                hit(Event::ThumbChange);
            }
        }
        self.base = None;
        hit(Event::FullSave);
        let mut seed = Seed::new(thumbs.storage_bytes());
        super::save_compact_recorded(state, path, seed.as_mut(), &thumbs)?;
        if let Some(mut seed) = seed {
            if let (Some(path), Some(stamp), Some(mut raw)) =
                (seed.path.take(), seed.stamp.take(), seed.raw.take())
            {
                if let Some(index) = Index::new(&mut raw) {
                    let mut reservation = seed.reservation;
                    let retained =
                        index.bytes() + budget::EDITS + thumbs.storage_bytes() + budget::IO;
                    if retained > reservation.0 {
                        hit(Event::SeedCharge);
                        return Ok(());
                    }
                    reservation.shrink(retained);
                    self.base = Some(Base {
                        raw,
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
                    hit(Event::SeedReady);
                } else {
                    hit(Event::SeedIndex);
                }
            } else {
                hit(Event::SeedSource);
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
    fn current_matches(&mut self, span: Span, expected: &[u8]) -> Option<()> {
        if let Some(bytes) = self.replacements.get(&span) {
            return (bytes.as_slice() == expected).then_some(());
        }
        if span.end.checked_sub(span.start)? != expected.len() {
            return None;
        }
        // Only the bounded typed metadata/header pieces are read here. Complete
        // descendant bytes are never copied into a metadata comparison buffer.
        let mut bytes = vec![0; expected.len()];
        self.raw
            .seek(SeekFrom::Start(span.start.try_into().ok()?))
            .ok()?;
        self.raw.read_exact(&mut bytes).ok()?;
        (bytes == expected).then_some(())
    }
    fn patch(&mut self, old: &PenNode, current: &PenNode) -> Option<()> {
        let node = self.index.nodes.get(old.id_str()).copied().flatten()?;
        // visit_metadata_changes proved the old SharedNode child wire witness
        // equals the current node's presence, before exposing either header.
        let before = splice::old_metadata(old, current.children().is_some())?;
        let after = splice::metadata(current)?;
        let old_parts = splice::parts(&before)?;
        let new_parts = splice::parts(&after)?;
        match (node.children, old_parts, new_parts) {
            (None, None, None) => {
                self.current_matches(node.object, &before)?;
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
                self.current_matches(prefix, &before[..a.start])?;
                self.current_matches(suffix, &before[a.end..])?;
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
                    copy_range(&mut self.raw, at, span.start, writer)?;
                    writer
                        .write_all(bytes)
                        .map_err(|e| DocIoError::Io(e.to_string()))?;
                    at = span.end;
                }
                let end = self
                    .raw
                    .metadata()
                    .map_err(|e| DocIoError::Io(e.to_string()))?
                    .len();
                copy_range(
                    &mut self.raw,
                    at,
                    usize::try_from(end).map_err(|e| DocIoError::Io(e.to_string()))?,
                    writer,
                )
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
/// The writer seals this descriptor after flushing its sibling staging file,
/// before the existing identity-checked atomic rename. Reading an arbitrary
/// later destination never establishes canonical byte provenance.
pub(super) struct Seed {
    raw: Option<File>,
    reservation: budget::Reservation,
    path: Option<PathBuf>,
    stamp: Option<FileVersion>,
}
impl Seed {
    fn new(thumbnails: usize) -> Option<Self> {
        hit(Event::SeedAttempt);
        let Some(reservation) = budget::Reservation::new(thumbnails) else {
            hit(Event::SeedBudget);
            return None;
        };
        Some(Self {
            raw: None,
            reservation,
            path: None,
            stamp: None,
        })
    }
    pub(super) fn bind(&mut self, path: &Path) {
        self.path = Some(path.to_path_buf());
    }
    pub(super) fn staged(&mut self, path: &Path) {
        if let Ok(file) = File::open(path) {
            if let Ok(meta) = file.metadata() {
                if usize::try_from(meta.len())
                    .is_ok_and(|bytes| self.reservation.reserve_source(bytes))
                {
                    self.stamp = Some(FileVersion::metadata(&meta));
                    self.raw = Some(file);
                }
            }
        }
    }
    pub(super) fn writer<'a, W: Write>(&'a mut self, writer: &'a mut W) -> &'a mut W {
        writer
    }
}
fn copy_range(
    source: &mut File,
    start: usize,
    end: usize,
    writer: &mut impl Write,
) -> Result<(), DocIoError> {
    let len = end
        .checked_sub(start)
        .ok_or_else(|| DocIoError::Io("invalid canonical source range".into()))?;
    source
        .seek(SeekFrom::Start(start.try_into().map_err(
            |e: std::num::TryFromIntError| DocIoError::Io(e.to_string()),
        )?))
        .map_err(|e| DocIoError::Io(e.to_string()))?;
    let copied = std::io::copy(&mut source.take(len as u64), writer)
        .map_err(|e| DocIoError::Io(e.to_string()))?;
    if copied != len as u64 {
        return Err(DocIoError::Io(
            "canonical source range became truncated".into(),
        ));
    }
    Ok(())
}
