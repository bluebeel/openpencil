//! Headless `.pen` / `.op` document load / save — the host-free core
//! carved out of `op-host-desktop`'s `persistence.rs` (Phase 2, Task
//! 2.3). The rfd / winit dialog flow (`handle_open` / `handle_save` /
//! `run_action` / `show_error_dialog`) stays desktop-side and imports
//! these functions back; the web daemon and any other headless caller
//! reach the same serializer the GUI uses.
//!
//! Save serializes `EditorState::doc` straight to canonical `.op` JSON
//! (the same schema the TS editor / Jian apps emit); Open parses the
//! canonical schema and re-seeds `EditorState` via
//! `EditorState::from_document`. There is no `Document → PenDocument`
//! reverse path, so the desktop's old private `DocPayload` format is no
//! longer written — every save is canonical.
//!
//! ## Embedded editor view-state (`editorMeta`)
//!
//! The canonical `PenDocument` schema has no field for editor
//! view-state — most of it (selection / viewport / tool) is deliberately
//! transient. `active_page_index` and the document-derived
//! `preserve_authored_geometry` layout latch must survive a save / load
//! round-trip, so Save embeds them under a top-level `editorMeta` extension.
//! Older files that still have the former `<path>.opmeta` sidecar continue to
//! load best-effort; new saves remove stale sidecars.
//!
//! ## Legacy `DocPayload` files
//!
//! Pre-canonical desktop builds saved a private `DocPayload` JSON
//! (`{"version": 1, "active_page_index": …, "pages": […]}` — note the
//! integer `version`). There is no `DocPayload → PenDocument`
//! converter (that needs a full node-tree converter, out of scope for
//! this fix), so rather than fail silently with an opaque schema
//! error, [`load_editor_state`] detects the legacy shape and surfaces
//! an explicit "saved by an older version, must be re-saved" message.

use std::path::PathBuf;

use op_editor_core::EditorState;

pub mod atomic_file;
mod canonical_save;
mod clean_copy;
#[cfg(test)]
mod compact_save_tests;
#[cfg(test)]
mod editor_meta_roundtrip_tests;
mod error;
#[cfg(test)]
mod hosted_catalog_tests;
pub(crate) mod incremental;
mod load;
mod load_report;
#[cfg(test)]
mod load_report_tests;
mod save_buffer;
#[cfg(test)]
#[path = "doc_io/work_family_tests.rs"]
mod work_family_tests;

use atomic_file::{create_sibling_temp, replace_file};
pub use canonical_save::{
    canonical_document_bytes, write_canonical_document, CanonicalSaveSnapshot, StreamingSaveStats,
};
pub use clean_copy::{
    copy_clean_document_with_editor_meta_to_path, copy_document_to_current_schema_path,
};
pub use error::DocIoError;
pub(crate) use load::load_editor_state_resolved;
#[cfg(test)]
use load::looks_like_legacy_doc_payload;
pub use load::{
    load_editor_state, load_editor_state_from_source, load_editor_state_with_report,
    load_editor_state_without_thumbnails,
};
pub use load_report::{DocumentLoadReport, LoadedEditorState};

/// Legacy sidecar path for a given `.op` / `.pen` file —
/// `<path>.opmeta`. Public so compatibility tests and stale-sidecar
/// cleanup use the same path convention older builds wrote.
pub fn sidecar_path(path: &std::path::Path) -> PathBuf {
    let mut p = path.to_path_buf();
    let ext = match path.extension().and_then(|s| s.to_str()) {
        Some(ext) => format!("{}.opmeta", ext),
        None => "opmeta".to_string(),
    };
    p.set_extension(ext);
    p
}

/// Serialize an `EditorState`'s canonical document to `path` without
/// prompting. Used by Cmd+S once the document already has a path.
/// Editor metadata is embedded under the top-level `editorMeta` extension so
/// the active page and Preserve-import geometry mode survive the round-trip
/// without a separate sidecar.
pub fn save_to_path(state: &EditorState, path: &std::path::Path) -> Result<(), DocIoError> {
    let thumbnails = jian_ops_schema::image_thumbs::capture_snapshot();
    save_document_with_thumbnails_to_path(
        &state.doc,
        op_pen_loader::EditorMeta::from_state(state),
        &thumbnails,
        path,
    )
}

/// Atomically save an agent edit with compact JSON. The canonical values,
/// image tables and editor metadata are identical to an explicit pretty save.
pub fn save_to_path_compact(state: &EditorState, path: &std::path::Path) -> Result<(), DocIoError> {
    let thumbnails = jian_ops_schema::image_thumbs::capture_snapshot();
    save_serializable_document_with_format(
        &state.doc,
        op_pen_loader::EditorMeta::from_state(state),
        &thumbnails,
        path,
        SaveFormat::Compact,
    )
}

/// Save WITHOUT capturing the process-global thumbnail registry.
///
/// For the multi-tenant online daemon. `capture_snapshot` reads a registry
/// with no tenant dimension, so an eviction would write whatever account
/// happened to have activated last into THIS account's file — persisting
/// another tenant's image data. Online policy disables thumbnails anyway, so
/// the file simply carries none.
pub fn save_to_path_without_thumbnails(
    state: &EditorState,
    path: &std::path::Path,
) -> Result<(), DocIoError> {
    save_document_with_thumbnails_to_path(
        &state.doc,
        op_pen_loader::EditorMeta::from_state(state),
        &jian_ops_schema::image_thumbs::ImageThumbSnapshot::default(),
        path,
    )
}

/// Save a borrowed canonical document without first cloning it. Synchronous
/// callers use this path; background desktop jobs should capture a
/// [`CanonicalSaveSnapshot`] on the UI thread and call
/// [`save_snapshot_to_path`] in their worker instead.
pub fn save_document_to_path(
    document: &jian_ops_schema::PenDocument,
    active_page_index: usize,
    path: &std::path::Path,
) -> Result<(), DocIoError> {
    let thumbnails = jian_ops_schema::image_thumbs::capture_snapshot();
    // This document-only compatibility entry point predates the authored-
    // geometry latch. Without an EditorState there is no truthful value to
    // capture, so every other metadata field retains its legacy default.
    save_document_with_thumbnails_to_path(
        document,
        op_pen_loader::EditorMeta {
            active_page_index,
            ..op_pen_loader::EditorMeta::default()
        },
        &thumbnails,
        path,
    )
}

/// Save a self-contained snapshot captured for a background job.
pub fn save_snapshot_to_path(
    snapshot: &CanonicalSaveSnapshot,
    path: &std::path::Path,
) -> Result<(), DocIoError> {
    save_serializable_document_with_thumbnails_to_path(
        snapshot.document(),
        snapshot.editor_meta(),
        snapshot.image_thumbnails(),
        path,
    )
}

/// Save a captured document and its matching thumbnail registry. Callers
/// retaining an outgoing document must not capture the incoming registry.
pub fn save_document_with_thumbnails_to_path(
    document: &jian_ops_schema::PenDocument,
    meta: op_pen_loader::EditorMeta,
    thumbnails: &jian_ops_schema::image_thumbs::ImageThumbSnapshot,
    path: &std::path::Path,
) -> Result<(), DocIoError> {
    save_serializable_document_with_thumbnails_to_path(document, meta, thumbnails, path)
}

fn save_serializable_document_with_thumbnails_to_path<
    D: serde::Serialize + jian_ops_schema::image_table::SaveImageOrder + ?Sized,
>(
    document: &D,
    meta: op_pen_loader::EditorMeta,
    thumbnails: &jian_ops_schema::image_thumbs::ImageThumbSnapshot,
    path: &std::path::Path,
) -> Result<(), DocIoError> {
    save_serializable_document_with_format(document, meta, thumbnails, path, SaveFormat::Pretty)
}

#[derive(Clone, Copy)]
enum SaveFormat {
    Pretty,
    Compact,
}

fn save_serializable_document_with_format<
    D: serde::Serialize + jian_ops_schema::image_table::SaveImageOrder + ?Sized,
>(
    document: &D,
    meta: op_pen_loader::EditorMeta,
    thumbnails: &jian_ops_schema::image_thumbs::ImageThumbSnapshot,
    path: &std::path::Path,
    format: SaveFormat,
) -> Result<(), DocIoError> {
    save_serializable_document_with_format_and_seed(document, meta, thumbnails, path, format, None)
}

fn save_compact_recorded(
    state: &EditorState,
    path: &std::path::Path,
    seed: Option<&mut incremental::Seed>,
    thumbnails: &jian_ops_schema::image_thumbs::ImageThumbSnapshot,
) -> Result<(), DocIoError> {
    save_serializable_document_with_format_and_seed(
        &state.doc,
        op_pen_loader::EditorMeta::from_state(state),
        thumbnails,
        path,
        SaveFormat::Compact,
        seed,
    )
}

fn save_serializable_document_with_format_and_seed<
    D: serde::Serialize + jian_ops_schema::image_table::SaveImageOrder + ?Sized,
>(
    document: &D,
    meta: op_pen_loader::EditorMeta,
    thumbnails: &jian_ops_schema::image_thumbs::ImageThumbSnapshot,
    path: &std::path::Path,
    format: SaveFormat,
    mut seed: Option<&mut incremental::Seed>,
) -> Result<(), DocIoError> {
    // Write through a sibling temp file so a crash mid-write doesn't
    // leave a half-written file on disk.
    let authorization =
        crate::mcp_serve::file_ownership::authorize_save(path).map_err(|error| {
            DocIoError::FileOwnership {
                detail: error.to_string(),
                committed: false,
            }
        })?;
    let _operation = authorization.scope();
    let canonical = authorization.path().to_path_buf();
    let path = canonical.as_path();
    if let Some(seed) = seed.as_deref_mut() {
        seed.bind(path);
    }
    let (tmp, file) = create_sibling_temp(path)?;
    let write_result = (|| {
        let serialize = |writer: &mut std::io::BufWriter<save_buffer::SaveTarget>| match format {
            SaveFormat::Pretty => canonical_save::write_serializable_document_with_thumbnails(
                writer, document, meta, thumbnails,
            ),
            SaveFormat::Compact => {
                if let Some(seed) = seed.as_deref_mut() {
                    canonical_save::write_compact_document_with_thumbnails(
                        &mut seed.writer(writer),
                        document,
                        meta,
                        thumbnails,
                    )
                } else {
                    canonical_save::write_compact_document_with_thumbnails(
                        writer, document, meta, thumbnails,
                    )
                }
            }
        };
        match format {
            SaveFormat::Pretty => {
                save_buffer::with_buffered_file(file, serialize)?;
            }
            SaveFormat::Compact => {
                save_buffer::with_shared_buffered_file(file, serialize)?;
            }
        }
        if let Some(seed) = seed.as_deref_mut() {
            seed.staged(&tmp);
        }
        commit_staged_document(&tmp, path)
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    write_result
}

/// Atomically install a completed sibling `.op` file at its fixed destination.
///
/// Figma import writes the large document to a hidden sibling first so it can
/// still observe cancellation before publication. Reusing the save path's
/// replace primitive keeps re-imports crash-safe while allowing the existing
/// `<source-stem>.op` to be replaced instead of creating numbered copies.
pub fn commit_staged_document(
    staging_path: &std::path::Path,
    destination: &std::path::Path,
) -> Result<(), DocIoError> {
    let mut authorization =
        crate::mcp_serve::file_ownership::authorize_save(destination).map_err(|error| {
            DocIoError::FileOwnership {
                detail: error.to_string(),
                committed: false,
            }
        })?;
    let canonical = authorization.path().to_path_buf();
    authorization
        .staging_ready(staging_path)
        .map_err(|error| DocIoError::FileOwnership {
            detail: error.to_string(),
            committed: false,
        })?;
    replace_file(staging_path, &canonical)?;
    authorization
        .committed()
        .map_err(|error| DocIoError::FileOwnership {
            detail: format!(
                "unconfirmed committed save; inspect persisted state before retrying: {error}"
            ),
            committed: true,
        })?;
    // Old builds wrote `<path>.opmeta`. New saves are single-file, so
    // remove any stale sidecar after the document write has committed.
    match std::fs::remove_file(sidecar_path(&canonical)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            eprintln!("[save] stale view-state sidecar cleanup failed: {e}");
        }
    }
    Ok(())
}

/// A cheap content fingerprint of the document — the hash of its
/// canonical JSON serialization. The desktop runner compares the
/// live fingerprint against the one captured at the last save /
/// open / new to decide whether there are unsaved changes worth a
/// close-time prompt. A serialization failure (not expected for a
/// valid document) yields a sentinel that simply reads as "changed".
pub fn document_fingerprint(state: &EditorState) -> u64 {
    use std::hash::Hasher;
    // Fingerprints are transient editor state, so their hash has no on-disk
    // compatibility contract. Stream the canonical bytes instead of retaining
    // a second serialized copy of a potentially image-heavy document.
    struct HashWriter(std::collections::hash_map::DefaultHasher);
    impl std::io::Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.write(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter(std::collections::hash_map::DefaultHasher::new());
    match serde_json::to_writer(&mut writer, &state.doc) {
        Ok(()) => writer.0.finish(),
        Err(_) => 0,
    }
}

/// Set `editor_ui.file_name_display` from a path's file name (or clear
/// it for an unsaved document). Public because both the carved load
/// path and the desktop residual's `set_display_name` host wrapper
/// call it (codex Issue 4 — shared helper).
pub fn set_file_name_display(state: &mut EditorState, path: Option<&std::path::Path>) {
    state.editor_ui.file_name_display = path
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned());
}

/// Carry app-level preferences from the previously open document onto a
/// freshly loaded / reset one (theme / locale / recents / agent config
/// / UIKits / theme presets / chat model selection). `from_document`
/// resets these to built-ins, so the New / Open paths re-apply them.
/// Public — called by both the carved load path and the desktop
/// residual's `run_action` New branch (codex Issue 4 — shared helper).
pub fn preserve_app_preferences(previous: &EditorState, next: &mut EditorState) {
    let previous_selected_model = previous.chat.selected_model_entry().cloned();
    next.editor_ui.theme_mode = previous.editor_ui.theme_mode;
    next.editor_ui.account_ui_available = previous.editor_ui.account_ui_available;
    next.editor_ui.account = previous.editor_ui.account.clone();
    next.editor_ui.locale = previous.editor_ui.locale;
    next.editor_ui.entry_surface = previous.editor_ui.entry_surface;
    next.editor_ui.recent_files = previous.editor_ui.recent_files.clone();
    next.editor_ui.font_import_supported = previous.editor_ui.font_import_supported;
    next.editor_ui.batch_frame_export_supported = previous.editor_ui.batch_frame_export_supported;
    next.editor_ui.scene_template_center.save_current_supported = previous
        .editor_ui
        .scene_template_center
        .save_current_supported;
    next.editor_ui.deck_html_export_supported = previous.editor_ui.deck_html_export_supported;
    next.editor_ui.slide_thumbnails_supported = previous.editor_ui.slide_thumbnails_supported;
    next.editor_ui.scene_template_generate_supported =
        previous.editor_ui.scene_template_generate_supported;
    next.editor_ui.system_fonts_loaded = previous.editor_ui.system_fonts_loaded;
    next.editor_ui.system_font_families = previous.editor_ui.system_font_families.clone();
    next.editor_ui.bundled_font_families = previous.editor_ui.bundled_font_families.clone();
    next.editor_ui.imported_font_families = previous.editor_ui.imported_font_families.clone();
    next.editor_ui.prompt_center.custom_prompts =
        previous.editor_ui.prompt_center.custom_prompts.clone();
    next.editor_ui.prompt_center.custom_store_writable =
        previous.editor_ui.prompt_center.custom_store_writable;
    next.editor_ui.prompt_center.custom_store_dirty =
        previous.editor_ui.prompt_center.custom_store_dirty;
    // Imported UIKits are app-level (persisted in `uikits.json`), not
    // document state — `from_document` reset them to the built-ins.
    next.ui_kits = previous.ui_kits.clone();
    // #20: theme presets are app-level too (`theme-presets.json`).
    next.theme_presets = previous.theme_presets.clone();
    next.theme_presets_dirty = previous.theme_presets_dirty;
    // Entering a document (Open / recent / drop / template / New)
    // starts the AI panel as its compact input bar, so the first thing
    // the user sees is the document rather than a chat panel over it. A
    // turn still streaming in the previous document keeps the panel
    // open — hiding a reply mid-flight reads as having lost it.
    if !previous.chat.has_streaming_turn() {
        next.chat.minimize();
    }
    next.editor_ui.agent_settings = previous.editor_ui.agent_settings.clone();
    next.editor_ui.chat_selected_agent = previous.editor_ui.chat_selected_agent;
    next.chat.discovered_models = previous.chat.discovered_models.clone();
    let _ = next.replace_hosted_model_catalog(previous.editor_ui.hosted_model_routes().to_vec());
    next.rebuild_chat_models();
    if let Some(prev) = previous_selected_model {
        if let Some(idx) = next.chat.available_models.iter().position(|m| {
            m.provider == prev.provider
                && m.value == prev.value
                && m.builtin_provider_id == prev.builtin_provider_id
                && m.hosted_route_id() == prev.hosted_route_id()
        }) {
            next.select_chat_model(idx);
        }
    }
}

/// Layout-resolved content bounding box of the active page (min_x, min_y,
/// max_x, max_y) in document coordinates, or `None` for an empty page.
///
/// The canonical document can store keyword sizing such as `fit_content`, so
/// raw authored bounds are insufficient here: a content-sized root has a raw
/// height of zero even though the canvas lays it out to its children's height.
/// Keep open diagnostics on the same resolved scene geometry used by canvas
/// framing and raster export.
pub fn active_page_bbox(state: &EditorState) -> Option<(f64, f64, f64, f64)> {
    let bounds = op_pen_loader::editor_state_to_active_page_layout_scene(state).content_bounds()?;
    let min_x = f64::from(bounds.origin.x);
    let min_y = f64::from(bounds.origin.y);
    Some((
        min_x,
        min_y,
        min_x + f64::from(bounds.size.x),
        min_y + f64::from(bounds.size.y),
    ))
}

/// Whether `path` carries a document extension this build opens
/// (`.op` / `.pen`). Used to filter drag-and-drop drops + the
/// file-association argv before attempting a load. The extension
/// match is case-insensitive so `MyDesign.OP` from a case-folding
/// filesystem / argv still opens.
pub fn is_supported_document(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("op") || ext.eq_ignore_ascii_case("pen"))
}

/// True for Figma `.fig` binary exports. The bundle declares `.fig`
/// as a `CFBundleDocumentTypes` extension (macOS / Windows / Linux),
/// so double-clicking one in Finder / dragging one onto the dock /
/// the running window all need to route through
/// `figma_import_session::spawn_approved` rather than the `.op`-only
/// `open_path`. Case-insensitive (Figma's "Save Local Copy" emits
/// `.fig`; some macOS shares fold to `.FIG`).
pub fn is_supported_figma_import(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("fig"))
}

/// True for HTML pages (`.html` / `.htm`) and packaged HTML projects
/// (`.zip`) — routed through the desktop `html_import_session`
/// (op-html structured import) rather than the `.op`-only `open_path`.
/// Case-insensitive like the other extension filters.
pub fn is_supported_html_import(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("html")
                || ext.eq_ignore_ascii_case("htm")
                || ext.eq_ignore_ascii_case("zip")
        })
}

/// Outcome of the desktop residual's `run_action` — tells the desktop
/// runner which post-action bookkeeping to run. Lives here (not on the
/// desktop side) so the headless daemon can name it too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionOutcome {
    /// The document now matches a file on disk (New / successful
    /// Open / Save / Save-As / Open-Recent). The runner refreshes the
    /// unsaved-changes baseline AND rebinds the Git session.
    Saved,
    /// User picked a `.fig` and the desktop runner should spawn the
    /// background parser (`figma_import_session::spawn_approved`). The actual
    /// document swap happens later when `figma_import_session::pump`
    /// drains the worker's result + rebinds the Git session itself
    /// (the previously-open repo binding goes stale on import).
    FigmaImportStarted(PathBuf),
    /// User chose a page, all pages, or cancel in the prepared Figma
    /// page selector. The desktop runner owns and consumes the session.
    FigmaImportSelection(op_editor_core::FigmaImportSelection),
    /// User picked a saved web page or ZIP project and the desktop
    /// runner should spawn the background HTML import worker
    /// (`html_import_session::spawn`), which applies the parsed document
    /// when it lands.
    HtmlImportStarted(PathBuf),
    /// A legacy synchronous Save-As entry reached a collaboration-bound
    /// document. The desktop runner must schedule its background Save-As fork
    /// flow; synchronous persistence has no authority to detach the session.
    SaveAsForkRequired,
    /// Nothing to reconcile — export, recent-list edits, or a user
    /// cancel / error.
    Noop,
}

impl ActionOutcome {
    /// Map a save/open helper's `bool` (`true` = the document now
    /// matches a file on disk) onto an outcome. Public — the desktop
    /// residual's `run_action` calls it (codex Issue 4 — shared helper).
    pub fn saved_or_noop(saved: bool) -> Self {
        if saved {
            ActionOutcome::Saved
        } else {
            ActionOutcome::Noop
        }
    }
}

/// Which file operation produced an error — picks the error dialog's
/// title / lead copy. Lives here so the headless callers and the
/// desktop `show_error_dialog` agree on the variant set.
#[derive(Debug, Clone, Copy)]
pub enum ErrorKind {
    Open,
    Save,
    Export,
}

#[cfg(test)]
mod tests;
