//! Scene-aware repeatable painting. Public FnOnce canvas callbacks stay native.

use super::{correlated_raster, ExportError, RasterFormat, RasterSampling};
use op_editor_ui::{
    layout_scene::{LayoutScene, SceneNode, ScenePage},
    Rect,
};
use std::path::Path;

pub fn render_scene_nodes_raster_bytes(
    nodes: &[SceneNode],
    bounds: Rect,
    format: RasterFormat,
    scale: f32,
    margin: f32,
    sampling: RasterSampling,
) -> Result<Vec<u8>, ExportError> {
    let scale = super::raster_scale(scale);
    match sampling {
        RasterSampling::Native => {
            super::render_raster_bytes(bounds, format, scale, margin, |canvas| {
                super::paint_nodes(canvas, nodes)
            })
        }
        RasterSampling::Correlated4x2 => {
            format.ensure_supported()?;
            correlated_raster::dimensions(bounds, scale, margin)?;
            super::scene_painter::with_prepared_nodes(nodes, |paint| {
                correlated_raster::render(bounds, format, scale, margin, paint)
            })
        }
    }
}

pub fn render_page_raster_bytes_with_sampling(
    page: &ScenePage,
    format: RasterFormat,
    scale: f32,
    sampling: RasterSampling,
) -> Result<Vec<u8>, ExportError> {
    if sampling == RasterSampling::Native {
        return super::render_page_raster_bytes(page, format, scale);
    }
    let bounds = super::page_bounds(page).ok_or_else(|| ExportError::PageEmpty {
        page_id: page.id.clone(),
    })?;
    render_scene_nodes_raster_bytes(&page.children, bounds, format, scale, 0.0, sampling)
}

pub fn render_node_on_page_raster_bytes_with_sampling(
    page: &ScenePage,
    node_id: &str,
    format: RasterFormat,
    scale: f32,
    sampling: RasterSampling,
) -> Result<Vec<u8>, ExportError> {
    if sampling == RasterSampling::Native {
        return super::render_node_on_page_raster_bytes(page, node_id, format, scale);
    }
    let batch = super::SceneSelectionBatch::new(page, [node_id]);
    let selected = batch
        .get(node_id)
        .ok_or_else(|| ExportError::NodeNotFoundOnPage {
            node_id: node_id.into(),
            page_id: page.id.clone(),
        })?;
    super::render_selected_node_raster_bytes_with_sampling(&selected, format, scale, sampling)
}

/// Raw isolated-node API. Use the selected-node variant for ancestor context.
pub fn render_scene_node_raster_bytes_with_sampling(
    node: &SceneNode,
    format: RasterFormat,
    scale: f32,
    sampling: RasterSampling,
) -> Result<Vec<u8>, ExportError> {
    if sampling == RasterSampling::Native {
        return super::render_scene_node_raster_bytes(node, format, scale);
    }
    if node.hidden {
        return Err(ExportError::NodeHidden {
            node_id: node.id.clone(),
        });
    }
    let bounds = super::node_bounds(node).ok_or_else(|| ExportError::NodePaintsNothing {
        node_id: node.id.clone(),
    })?;
    render_scene_nodes_raster_bytes(
        std::slice::from_ref(node),
        bounds,
        format,
        scale,
        0.0,
        sampling,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn export_node_raster_with_viewport_and_sampling(
    scene: &LayoutScene,
    node_id: &str,
    target: &Path,
    format: RasterFormat,
    scale: f32,
    viewport: Rect,
    sampling: RasterSampling,
) -> Result<(), ExportError> {
    if sampling == RasterSampling::Native {
        return super::export_node_raster_with_viewport(
            scene, node_id, target, format, scale, viewport,
        );
    }
    let page = scene.active_page().ok_or(ExportError::NoActivePage)?;
    let batch = super::SceneSelectionBatch::new(page, [node_id]);
    let selected = batch
        .get(node_id)
        .ok_or_else(|| ExportError::NodeNotFoundOnActivePage {
            node_id: node_id.into(),
        })?;
    let bytes =
        super::render_selected_node_in_viewport(&selected, viewport, format, scale, 0.0, sampling)?;
    std::fs::write(target, bytes).map_err(|error| ExportError::Write(error.to_string()))
}
