//! Exact, authority-owned incremental compact saves. Completed staging files
//! back unchanged ranges; only completed metadata receipts advance a chain.
use super::DocIoError;
use crate::mcp_serve::file_ownership::FileVersion;
use jian_ops_schema::{image_thumbs::ImageThumbSnapshot, node::PenNode};
use op_editor_core::{CanonicalHeaderChange, EditorState, PenNodeExt};
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
mod append;
pub(crate) use stats::snapshot as snapshot_stats;
use stats::{hit, Event};
pub(crate) use tracked::TrackedEditor;
pub(crate) const HEADER_VALUE_LIMIT: usize = 4 * 1024 * 1024;

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
    appends: append::Overlay,
    replacement_bytes: usize,
    path: PathBuf,
    stamp: FileVersion,
    owner: std::sync::Weak<()>,
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
            base.owner.ptr_eq(&state.derived_cache_owner())
                && base.generation == state.document_generation()
                && base.revision == state.document_revision()
        })
    }
    /// Call immediately after each completed command, before another command.
    /// A false proof, changed IDs/resources, partial visit or bounds exhaustion
    /// discards the whole derived cache; the canonical writer remains fallback.
    pub(crate) fn observe<'a>(
        &mut self,
        state: &'a EditorState,
        visit: impl FnOnce(
            &mut dyn FnMut(&PenNode, &PenNode),
            &mut dyn FnMut(CanonicalHeaderChange) -> bool,
            &mut dyn FnMut(op_editor_core::CanonicalAppend<'a>) -> bool,
        ) -> bool,
    ) {
        let Some(base) = self.base.as_mut() else {
            return;
        };
        // All visitors share one transaction. If any piece refuses, discard
        // the entire derived chain before the ordinary canonical writer runs.
        let valid = std::cell::Cell::new(
            base.owner.ptr_eq(&state.derived_cache_owner())
                && base.generation == state.document_generation(),
        );
        let exact = {
            let base = std::cell::RefCell::new(base);
            visit(
                &mut |old, current| {
                    if !valid.get() {
                        return;
                    }
                    let okay = old.id_str() == current.id_str()
                        && base.borrow_mut().patch(old, current).is_some();
                    if !okay {
                        hit(Event::MetadataMismatch);
                        valid.set(false);
                    }
                },
                &mut |change| {
                    if !valid.get() {
                        return false;
                    }
                    let okay = base.borrow_mut().patch_header(change).is_some();
                    if !okay {
                        hit(Event::HeaderMismatch);
                        valid.set(false);
                    }
                    okay
                },
                &mut |receipt| {
                    if !valid.get() {
                        return false;
                    }
                    let okay = base.borrow_mut().append(receipt).is_some();
                    if !okay {
                        hit(Event::AppendRefusal);
                        valid.set(false);
                    }
                    okay
                },
            )
        };
        if !exact || !valid.get() {
            if !exact {
                hit(Event::ReceiptRefusal);
            }
            self.invalidate();
        }
    }
    /// Collect the complete exact structural set before changing any array
    /// plans; nodes/headers use the same all-or-nothing observation as append.
    pub(crate) fn observe_deltas<'a>(
        &mut self,
        state: &'a EditorState,
        visit: impl FnOnce(
            &mut dyn FnMut(&PenNode, &PenNode),
            &mut dyn FnMut(CanonicalHeaderChange) -> bool,
            &mut dyn FnMut(op_editor_core::CanonicalSplice<'a>) -> bool,
        ) -> bool,
    ) {
        let mut packets = Vec::new();
        self.observe(state, |nodes, headers, _appends| {
            visit(nodes, headers, &mut |packet| {
                if packets.len() == 256 {
                    return false;
                }
                packets.push(packet);
                true
            })
        });
        if self
            .base
            .as_mut()
            .is_some_and(|base| base.apply_splices(&packets).is_none())
        {
            hit(Event::SpliceRefusal);
            self.invalidate();
        }
    }
    pub(crate) fn save(&mut self, state: &EditorState, path: &Path) -> Result<(), DocIoError> {
        self.save_inner(state, path, false).map(|_| ())
    }
    /// Explicit same-file save under a read-only authority scope. False means
    /// the exact canonical/view/resource state already has a confirmed durable
    /// commit. Different saved view metadata still performs a real byte copy.
    /// The caller must take this chain BEFORE obtaining mutable EditorState
    /// access and restore only after the audited read-only save scope ends.
    pub(crate) fn save_explicit(
        &mut self,
        state: &EditorState,
        path: &Path,
    ) -> Result<bool, DocIoError> {
        if !self.matches(state) {
            self.invalidate();
        }
        self.save_inner(state, path, true)
    }
    fn save_inner(
        &mut self,
        state: &EditorState,
        path: &Path,
        allow_matching_commit: bool,
    ) -> Result<bool, DocIoError> {
        let thumbs = jian_ops_schema::image_thumbs::capture_snapshot();
        if let Some(base) = &mut self.base {
            if base.owner.ptr_eq(&state.derived_cache_owner())
                && base.generation == state.document_generation()
                && base.thumbnails.same_contents(&thumbs)
            {
                if let Some(meta) =
                    splice::editor_meta(&op_pen_loader::EditorMeta::borrowed_from_state(state))
                {
                    let span = base.index.meta;
                    if allow_matching_commit
                        && base.revision == state.document_revision()
                        && base.current_matches(span, &meta).is_some()
                    {
                        match base.confirm_destination(path) {
                            Ok(true) => {
                                hit(Event::ExplicitCommitReuse);
                                return Ok(false);
                            }
                            Ok(false) => hit(Event::SourceDrift),
                            Err(error) => {
                                self.base = None;
                                return Err(error);
                            }
                        }
                    }
                    if base.replace(span, meta).is_some() {
                        // A real I/O/ownership failure is returned, never hidden
                        // by a second save attempt or an acknowledgement.
                        match base.write(path) {
                            Ok(true) => {
                                base.revision = state.document_revision();
                                hit(Event::IncrementalSave);
                                return Ok(true);
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
                        return Ok(true);
                    }
                    reservation.shrink(retained);
                    self.base = Some(Base {
                        raw,
                        index,
                        replacements: BTreeMap::new(),
                        appends: append::Overlay::default(),
                        replacement_bytes: 0,
                        path,
                        stamp,
                        owner: state.derived_cache_owner(),
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
        Ok(true)
    }
}
impl Base {
    /// Same ownership and actual FileVersion checks as a real byte-copy save,
    /// even when no new temporary file is needed. No replay or skipped auth.
    fn confirm_destination(&self, requested: &Path) -> Result<bool, DocIoError> {
        let authorization =
            crate::mcp_serve::file_ownership::authorize_save(requested).map_err(|error| {
                DocIoError::FileOwnership {
                    detail: error.to_string(),
                    committed: false,
                }
            })?;
        let _scope = authorization.scope();
        let path = authorization.path();
        let stamp = std::fs::metadata(path)
            .map(|meta| FileVersion::metadata(&meta))
            .map_err(|error| DocIoError::Io(error.to_string()))?;
        Ok(path == self.path && stamp == self.stamp)
    }

    fn patch_header(&mut self, change: CanonicalHeaderChange) -> Option<()> {
        let span = self.index.headers[change.field.index()]?;
        // Add/remove root keys needs delimiter/schema-order surgery. Use the
        // full writer for that first transition, then the next seed is indexed.
        let before = change.before?;
        let after = change.after?;
        self.current_matches(span, &before)?;
        self.replace(span, after)
    }

    fn replace(&mut self, span: Span, replacement: Vec<u8>) -> Option<()> {
        let old = self.replacements.get(&span).map_or(0, Vec::capacity);
        let bytes = self
            .replacement_bytes
            .checked_sub(old)?
            .checked_add(replacement.capacity())?;
        if (!self.replacements.contains_key(&span) && self.replacements.len() >= 8192)
            || self.appends.raw_overlap(span)
            || bytes.checked_add(self.appends.bytes())? > budget::EDITS - 1024 * 1024
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
            if span.overlaps(*other) {
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
        self.raw
            .seek(SeekFrom::Start(span.start.try_into().ok()?))
            .ok()?;
        let mut scratch = [0u8; 8192];
        for chunk in expected.chunks(scratch.len()) {
            self.raw.read_exact(&mut scratch[..chunk.len()]).ok()?;
            if &scratch[..chunk.len()] != chunk {
                return None;
            }
        }
        Some(())
    }
    fn patch(&mut self, old: &PenNode, current: &PenNode) -> Option<()> {
        if !append::eligible(old) || !append::eligible(current) {
            return None;
        }
        if self.appends.nodes.contains_key(old.id_str()) {
            return self.patch_appended(old, current);
        }
        let Some(node) = self.index.nodes.get(old.id_str()).copied().flatten() else {
            return self.patch_appended(old, current);
        };
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
                let children = children.span;
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
                self.appends
                    .write(&mut self.raw, &self.index, &self.replacements, writer)
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
