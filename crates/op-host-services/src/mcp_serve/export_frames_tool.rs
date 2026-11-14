//! MCP tools `export_frames` and `get_deck_boards`.
//!
//! `export_frames` writes every top-level frame on the active page into a
//! directory, one file each — the batch the desktop's Export panel has run
//! since `v0.8.3`, which an agent previously had to imitate by calling
//! `export_item` once per frame and inventing its own file names.
//!
//! Naming and filtering are not re-derived here: `plan_frame_exports` already
//! resolves collisions, caps long AI-authored names, and skips hidden frames
//! (the exporter paints them empty, so including them would only manufacture
//! failures). Re-implementing any of that would drift from what the panel
//! writes.
//!
//! `get_deck_boards` is the read half. Slideshow *control* is deliberately not
//! here: entering preview is a host mode transition, not document state, and a
//! file-backed MCP session has no window to present in. What an agent can use
//! is the board list a deck export will walk, so it can verify the deck before
//! writing one.

use std::collections::BTreeMap;
use std::path::PathBuf;

use op_editor_core::EditorState;
use op_mcp::{McpTool, ToolErrorCode, ToolOutcome};

use super::raster_snapshot::RasterSnapshot;
use crate::export_batch::{raster_file_targets, report_results};
use op_editor_core::export_batch::FrameExportTarget;

pub struct ExportFrames {
    scene: RasterSnapshot,
    targets: Vec<FrameExportTarget>,
    scale: f32,
}

impl McpTool for ExportFrames {
    fn name(&self) -> &str {
        "export_frames"
    }

    fn call(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        let directory = match args.get("outputDir").map(|value| value.trim()) {
            Some(directory) if !directory.is_empty() => PathBuf::from(directory),
            _ => {
                return ToolOutcome::Err(
                    ToolErrorCode::MissingArgument,
                    "outputDir is required — export_frames writes one file per frame".into(),
                );
            }
        };
        let extension = args
            .get("format")
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .unwrap_or("png");
        let Some(format) = crate::export::RasterFormat::from_extension(extension) else {
            return ToolOutcome::Err(
                ToolErrorCode::InvalidArgument,
                format!("unknown format {extension:?}: must be one of png, jpeg, webp"),
            );
        };

        if self.targets.is_empty() {
            return ToolOutcome::Err(
                ToolErrorCode::InvalidArgument,
                "the active page has no exportable top-level frames".into(),
            );
        }
        if let Err(error) = self.scene.ready() {
            return ToolOutcome::Err(ToolErrorCode::ToolFailed, error.to_string());
        }
        // Naming was planned once against the borrowed original state. Only
        // the final suffix depends on the encoder, preserving planner ordering,
        // selection, collision handling and names containing dots.
        let targets: Vec<_> = self
            .targets
            .iter()
            .map(|target| FrameExportTarget {
                node_id: target.node_id.clone(),
                file_name: format!(
                    "{}.{extension}",
                    target
                        .file_name
                        .strip_suffix(".png")
                        .expect("PNG export plan")
                ),
            })
            .collect();
        if let Err(error) = std::fs::create_dir_all(&directory) {
            return ToolOutcome::Err(
                ToolErrorCode::Internal,
                format!("cannot create {}: {error}", directory.display()),
            );
        }

        let results = match self.scene.export_files(
            raster_file_targets(&directory, &targets),
            format,
            self.scale,
        ) {
            Ok(results) => results,
            Err(error) => return ToolOutcome::Err(ToolErrorCode::ToolFailed, error.to_string()),
        };
        let report = report_results(&targets, results);
        // Partial success is reported, not raised: one unrenderable frame must
        // not throw away the files that did land.
        let failed: Vec<serde_json::Value> = report
            .failed
            .iter()
            .map(|(file_name, detail)| serde_json::json!({ "file": file_name, "error": detail }))
            .collect();
        ToolOutcome::OkJson(
            serde_json::json!({
                "directory": directory.display().to_string(),
                "format": extension,
                "attempted": report.attempted(),
                "written": report.written,
                "failed": failed,
            })
            .to_string(),
        )
    }
}

pub struct GetDeckBoards {
    result: String,
}

impl McpTool for GetDeckBoards {
    fn name(&self) -> &str {
        "get_deck_boards"
    }

    fn call(&self, _args: &BTreeMap<String, String>) -> ToolOutcome {
        ToolOutcome::OkJson(self.result.clone())
    }
}

pub fn export_frames_snapshot(state: &EditorState) -> ExportFrames {
    export_frames_snapshot_inner(state, false)
}

pub(super) fn export_frames_snapshot_admitted(state: &EditorState) -> ExportFrames {
    export_frames_snapshot_inner(state, true)
}

fn export_frames_snapshot_inner(state: &EditorState, admitted: bool) -> ExportFrames {
    let targets = op_editor_core::export_batch::plan_frame_exports(state, "png");
    let derive = || op_pen_loader::editor_state_to_active_page_export_layout_scene(state);
    let scene = if targets.is_empty() {
        RasterSnapshot::from_scene(op_editor_ui::layout_scene::LayoutScene {
            pages: Vec::new(),
            active_page_index: 0,
        })
    } else if admitted {
        RasterSnapshot::capture_http(
            state,
            op_pen_loader::ExportScenePolicy::ActivePresentation,
            derive,
        )
    } else {
        RasterSnapshot::from_scene(derive())
    };
    ExportFrames {
        scene,
        targets,
        scale: state.editor_ui.export_scale,
    }
}

pub fn get_deck_boards_snapshot(state: &EditorState) -> GetDeckBoards {
    let boards = op_editor_core::preview_slideshow::active_page_boards(state);
    GetDeckBoards {
        result: serde_json::json!({ "count": boards.len(), "boards": boards }).to_string(),
    }
}

#[cfg(test)]
#[path = "export_frames_tool_tests.rs"]
mod tests;
