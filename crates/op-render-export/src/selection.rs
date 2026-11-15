//! Context-preserving selected raster exports, shared by all native transports.

use crate::{ExportError, RasterFormat, RasterSampling};
pub use op_editor_ui::widgets::canvas_viewport_paint::{
    SceneSelection, SceneSelectionBatch, SceneSelectionCursor,
};
use op_editor_ui::Rect;
use std::path::Path;

pub fn selection_bounds(selection: &SceneSelection<'_>) -> Result<Rect, ExportError> {
    if !selection.is_visible() {
        return Err(ExportError::NodeHidden {
            node_id: selection.node().id.clone(),
        });
    }
    selection
        .paint_bounds()
        .ok_or_else(|| ExportError::NodePaintsNothing {
            node_id: selection.node().id.clone(),
        })
}

pub fn render_selected_node_raster_bytes(
    selection: &SceneSelection<'_>,
    format: RasterFormat,
    scale: f32,
) -> Result<Vec<u8>, ExportError> {
    render_selected_node_raster_bytes_with_sampling(
        selection,
        format,
        scale,
        RasterSampling::Native,
    )
}

pub fn render_selected_node_raster_bytes_with_sampling(
    selection: &SceneSelection<'_>,
    format: RasterFormat,
    scale: f32,
    sampling: RasterSampling,
) -> Result<Vec<u8>, ExportError> {
    format.ensure_supported()?;
    let bounds = selection_bounds(selection)?;
    render_selected_node_in_viewport(selection, bounds, format, scale, 0.0, sampling)
}

pub fn render_selected_node_in_viewport(
    selection: &SceneSelection<'_>,
    viewport: Rect,
    format: RasterFormat,
    scale: f32,
    margin: f32,
    sampling: RasterSampling,
) -> Result<Vec<u8>, ExportError> {
    format.ensure_supported()?;
    if !selection.is_visible() {
        return Err(ExportError::NodeHidden {
            node_id: selection.node().id.clone(),
        });
    }
    let scale = crate::raster_scale(scale);
    let margin = margin.max(0.0);
    let cull = Rect::xywh(
        viewport.origin.x - margin,
        viewport.origin.y - margin,
        viewport.size.x + margin * 2.0,
        viewport.size.y + margin * 2.0,
    );
    match sampling {
        RasterSampling::Native => {
            crate::render_raster_bytes(viewport, format, scale, margin, |canvas| {
                crate::scene_painter::paint_selection(canvas, selection, cull)
            })
        }
        RasterSampling::Correlated4x2 => {
            crate::correlated_raster::dimensions(viewport, scale, margin)?;
            crate::scene_painter::with_prepared_selection(selection, cull, |paint| {
                crate::correlated_raster::render(viewport, format, scale, margin, paint)
            })
        }
    }
}

pub fn export_selected_node_raster(
    selection: &SceneSelection<'_>,
    target: &Path,
    format: RasterFormat,
    scale: f32,
    margin: f32,
) -> Result<(), ExportError> {
    format.ensure_supported()?;
    let bounds = selection_bounds(selection)?;
    // This API always uses Native sampling. Encode the already-painted surface
    // straight into the existing atomic sibling writer instead of allocating
    // an encoded Vec and immediately copying it back into the file API.
    let scale = crate::raster_scale(scale);
    let margin = margin.max(0.0);
    let cull = Rect::xywh(
        bounds.origin.x - margin,
        bounds.origin.y - margin,
        bounds.size.x + margin * 2.0,
        bounds.size.y + margin * 2.0,
    );
    crate::render_raster(bounds, target, format, scale, margin, |canvas| {
        crate::scene_painter::paint_selection(canvas, selection, cull)
    })
}
