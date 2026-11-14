use std::path::Path;

use serde_json::json;

use super::{flag_value, required_pos, Command, Flags};
use crate::cli_error::CliError;

#[path = "figma_cli_save.rs"]
mod atomic_save;

pub(super) fn map_import_figma(positionals: &[String], flags: &Flags) -> Result<Command, CliError> {
    let fig_path = required_pos(
        positionals,
        1,
        "Usage: op import:figma <file.fig> [--out output.op]",
    )?;
    let out_path = flag_value(flags, "out").unwrap_or_else(|| figma_default_out_path(&fig_path));
    Ok(Command::ImportFigma { fig_path, out_path })
}

pub(super) fn figma_default_out_path(fig_path: &str) -> String {
    fig_path
        .strip_suffix(".fig")
        .map(|base| format!("{base}.op"))
        .unwrap_or_else(|| fig_path.to_string())
}

pub(super) fn run_import_figma(fig_path: &str, out_path: &str) -> Result<String, CliError> {
    let bytes =
        std::fs::read(fig_path).map_err(|e| CliError::Io(format!("read {fig_path:?}: {e}")))?;
    let file_name = Path::new(fig_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Figma Import");
    let import =
        op_figma::prepare_fig_binary_owned(bytes, file_name, op_figma::FigLayoutMode::OpenPencil)
            .and_then(|prepared| prepared.into_all_pages_with_images(None))
            .map_err(|e| CliError::Document(format!("import {fig_path:?}: {e}")))?;
    // Stream shared image payloads once, then atomically replace the destination.
    atomic_save::write_atomically(out_path, |output| {
        jian_ops_schema::image_table::write_document(
            output,
            &import.document,
            &jian_ops_schema::image_thumbs::capture_snapshot(),
        )
        .map_err(|e| {
            if e.is_io() {
                CliError::Io(format!("write {out_path:?}: {e}"))
            } else {
                CliError::Payload(format!("serialize {out_path:?}: {e}"))
            }
        })
    })?;
    let page_count = import.document.pages.as_ref().map_or(1, Vec::len);
    let node_count = import
        .document
        .pages
        .as_ref()
        .map(|pages| pages.iter().map(|p| p.children.len()).sum())
        .unwrap_or_else(|| import.document.children.len());
    Ok(import_figma_success_json(
        out_path,
        page_count,
        node_count,
        &import.warnings,
    ))
}

fn import_figma_success_json(
    file_path: &str,
    page_count: usize,
    node_count: usize,
    warnings: &[String],
) -> String {
    json!({
        "ok": true,
        "filePath": file_path,
        "pageCount": page_count,
        "nodeCount": node_count,
        "warnings": warnings,
    })
    .to_string()
}
