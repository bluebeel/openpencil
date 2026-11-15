//! File-only export transport: no throwaway artifact/encoded raster buffer.
use super::*;
pub(super) fn export(
    state: &EditorState,
    scope: &EditorExportScope,
    target: &Path,
) -> Result<(), ExportError> {
    if let EditorExportScope::DeckBoards(boards) = scope {
        return crate::pdf::export_deck_pdf_boards(state, target, boards);
    }
    let format = state.editor_ui.export_format;
    if format == ExportFormat::Webp && cfg!(any(target_os = "ios", target_os = "android")) {
        return Err(ExportError::UnsupportedFormat { format: "WEBP" });
    }
    if format == ExportFormat::Pdf {
        let app_pages = state.editor_ui.workspace.visible
            && state.editor_ui.workspace.family == HomeFamily::AppUi
            && !op_editor_core::preview_slideshow::active_page_boards(state).is_empty();
        if state.editor_ui.scenario == Some(TemplateScene::Slides) || app_pages {
            return crate::pdf::export_deck_pdf(state, target);
        }
        let scene = op_pen_loader::editor_state_to_export_layout_scene(state);
        return crate::pdf::export_pdf(&scene, target);
    }
    let scene = op_pen_loader::editor_state_to_active_page_export_layout_scene(state);
    let page = scene.active_page().ok_or(ExportError::NoActivePage)?;
    let node = (state.selection_count() == 1 && state.selection.anchor.is_real())
        .then(|| state.selection.anchor.as_str());
    let scale = state.editor_ui.export_scale;
    let format = match format {
        ExportFormat::Png => RasterFormat::Png,
        ExportFormat::Jpeg => RasterFormat::Jpeg,
        ExportFormat::Webp => RasterFormat::Webp,
        ExportFormat::Svg => {
            let bytes = match node {
                Some(id) => render_node_on_page_svg_bytes(page, id, scale)?,
                None => render_page_svg_bytes(page, scale)?,
            };
            return crate::encoding::write_bytes(target, &bytes);
        }
        ExportFormat::Pdf => unreachable!("PDF returned before scene construction"),
    };
    match node {
        Some(id) => {
            let batch = crate::SceneSelectionBatch::new(page, [id]);
            let selected = batch
                .get(id)
                .ok_or_else(|| ExportError::NodeNotFoundOnPage {
                    node_id: id.to_owned(),
                    page_id: page.id.clone(),
                })?;
            crate::export_selected_node_raster(&selected, target, format, scale, 0.0)
        }
        None => {
            let bounds = crate::page_bounds(page).ok_or_else(|| ExportError::PageEmpty {
                page_id: page.id.clone(),
            })?;
            crate::render_raster(
                bounds,
                target,
                format,
                crate::raster_scale(scale),
                0.0,
                |canvas| crate::paint_nodes(canvas, &page.children),
            )
        }
    }
}
