//! Selected PDF pages preserve the same borrowed native context as raster.

use super::{render_pdf_item, PDF_MARGIN};
use crate::{ExportError, SceneSelection, SceneSelectionBatch};

fn output_cull(bounds: op_editor_ui::Rect) -> op_editor_ui::Rect {
    op_editor_ui::Rect::xywh(
        bounds.origin.x - PDF_MARGIN,
        bounds.origin.y - PDF_MARGIN,
        bounds.size.x + PDF_MARGIN * 2.0,
        bounds.size.y + PDF_MARGIN * 2.0,
    )
}

pub fn render_selected_node_pdf_bytes_scaled(
    selection: &SceneSelection<'_>,
    scale: f32,
) -> Result<Vec<u8>, ExportError> {
    let bounds = crate::selection_bounds(selection)?;
    render_pdf_item(bounds, scale, |canvas| {
        crate::scene_painter::paint_selection(canvas, selection, output_cull(bounds))
    })
}

/// Combined PDF pages are rendered in input order, including duplicates.
/// Missing/hidden/unpainted selections retain the existing combined skip policy.
/// Only ID/bounds records are retained; context views and source paint remain
/// borrowed and are reconstructed from shared prefix links one page at a time.
pub fn render_selection_batch_pdf_bytes(
    batch: &SceneSelectionBatch<'_>,
    ids: &[&str],
    scale: f32,
) -> Result<Vec<u8>, ExportError> {
    crate::with_batch_preparation(|| render_batch_inner(batch, ids, scale))
}

fn render_batch_inner(
    batch: &SceneSelectionBatch<'_>,
    ids: &[&str],
    scale: f32,
) -> Result<Vec<u8>, ExportError> {
    if !scale.is_finite() || scale <= 0.0 {
        return Err(ExportError::NonFiniteOutputSize);
    }
    let mut cursor = batch.cursor();
    let pairs = ids
        .iter()
        .filter_map(|id| {
            let selection = cursor.get(batch, id)?;
            crate::selection_bounds(selection)
                .ok()
                .map(|bounds| (*id, bounds))
        })
        .collect::<Vec<_>>();
    if pairs.is_empty() {
        return Err(ExportError::NoRequestedNodesPaint);
    }
    if pairs.iter().any(|(_, b)| {
        !((b.size.x + PDF_MARGIN * 2.0) * scale).is_finite()
            || !((b.size.y + PDF_MARGIN * 2.0) * scale).is_finite()
    }) {
        return Err(ExportError::NonFiniteOutputSize);
    }
    let mut buffer = Vec::new();
    {
        let mut pdf = skia_safe::pdf::new_document(&mut buffer, None);
        for (id, bounds) in pairs {
            let Some(selection) = cursor.get(batch, id) else {
                continue;
            };
            let size = skia_safe::Size::new(
                (bounds.size.x + PDF_MARGIN * 2.0) * scale,
                (bounds.size.y + PDF_MARGIN * 2.0) * scale,
            );
            let mut page = pdf.begin_page(size, None);
            let canvas = page.canvas();
            canvas.scale((scale, scale));
            canvas.translate((PDF_MARGIN - bounds.origin.x, PDF_MARGIN - bounds.origin.y));
            crate::scene_painter::paint_selection(canvas, selection, output_cull(bounds));
            pdf = page.end_page();
        }
        pdf.close();
    }
    if buffer.is_empty() {
        Err(ExportError::PdfEncoderEmpty)
    } else {
        Ok(buffer)
    }
}
