//! One worker serializes producers, so queued identical targets naturally reuse
//! a completed success. Admission and each output/ACK remain the original ones.
use super::{
    encoded_cache::{Cache, Input},
    NodeBatchJob, RasterJob, SharedRasterTarget,
};
use crate::{ExportError, RasterSampling, SceneSelectionBatch};
use op_editor_ui::layout_scene::LayoutScene;
use std::{collections::HashSet, sync::Arc};
fn fonts() -> u64 {
    op_host_native::render_font_generation()
}
fn cached(cache: &mut Cache, scene: &Arc<LayoutScene>, input: Input<'_>) -> Option<Vec<u8>> {
    let bytes = cache.get(scene, input)?;
    (fonts() == input.fonts).then_some(bytes)
}
fn remember(
    cache: &mut Cache,
    scene: &Arc<LayoutScene>,
    input: Input<'_>,
    safe: bool,
    result: &Result<Vec<u8>, ExportError>,
) {
    if safe && fonts() == input.fonts {
        if let Ok(bytes) = result {
            cache.insert(scene, input, bytes);
        }
    }
}
pub(super) fn render_job(job: &RasterJob, cache: &mut Cache) -> Result<Vec<u8>, ExportError> {
    let page_id = match &job.target {
        SharedRasterTarget::Page { page_id } | SharedRasterTarget::Node { page_id, .. } => page_id,
    };
    let (slot, page) = job
        .scene
        .pages
        .iter()
        .enumerate()
        .find(|(_, page)| &page.id == page_id)
        .ok_or_else(|| ExportError::PageNotFound {
            page_id: page_id.clone(),
        })?;
    let input = Input {
        page: slot,
        node: match &job.target {
            SharedRasterTarget::Page { .. } => None,
            SharedRasterTarget::Node { node_id, .. } => Some(node_id),
        },
        format: job.format,
        scale: job.scale,
        sampling: job.sampling,
        fonts: fonts(),
    };
    if let Some(bytes) = cached(cache, &job.scene, input) {
        return Ok(bytes);
    }
    let input = Input {
        fonts: fonts(),
        ..input
    };
    let (safe, result) = match &job.target {
        SharedRasterTarget::Page { .. } => {
            let safe = page
                .children
                .iter()
                .all(|root| cache.resource_free(&job.scene, root));
            (
                safe,
                crate::render_page_raster_bytes_with_sampling(
                    page,
                    job.format,
                    job.scale,
                    job.sampling,
                ),
            )
        }
        SharedRasterTarget::Node { node_id, .. } => {
            // Preserve the Native API's unsupported-format error precedence.
            if job.sampling == RasterSampling::Native {
                job.format.ensure_supported()?;
            }
            let batch = SceneSelectionBatch::new(page, [node_id.as_str()]);
            let selected = batch
                .get(node_id)
                .ok_or_else(|| ExportError::NodeNotFoundOnPage {
                    node_id: node_id.clone(),
                    page_id: page.id.clone(),
                })?;
            let safe = selected.all_paint_sources(|root| cache.resource_free(&job.scene, root));
            (
                safe,
                crate::render_selected_node_raster_bytes_with_sampling(
                    &selected,
                    job.format,
                    job.scale,
                    job.sampling,
                ),
            )
        }
    };
    remember(cache, &job.scene, input, safe, &result);
    result
}
fn batch_input<'a>(job: &NodeBatchJob, id: &'a str, fonts: u64) -> Input<'a> {
    Input {
        page: job.scene.active_page_index,
        node: Some(id),
        format: job.format,
        scale: job.scale,
        sampling: RasterSampling::Native,
        fonts,
    }
}
pub(super) fn render_node_batch_job(job: NodeBatchJob, cache: &mut Cache) {
    let page = job.scene.active_page();
    let generation = fonts();
    // Only target identities are retained. Hot hits are not copied until their
    // own ordered send, so a thousand targets never produce a thousand buffers.
    let cold = job
        .node_ids
        .iter()
        .filter(|id| !cache.contains(&job.scene, batch_input(&job, id.as_str(), generation)))
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let nodes = page
        .filter(|_| !cold.is_empty())
        .map(|page| SceneSelectionBatch::new(page, cold.iter().copied()));
    let mut cursor = nodes.as_ref().map(|nodes| nodes.cursor());
    crate::with_batch_preparation(|| {
        for (index, id) in job.node_ids.iter().enumerate() {
            let key = batch_input(&job, id.as_str(), fonts());
            let result = if let Some(bytes) = cached(cache, &job.scene, key) {
                Ok(bytes)
            } else {
                let key = Input {
                    fonts: fonts(),
                    ..key
                };
                let mut safe = false;
                let result = match page {
                    None => Err(ExportError::NoActivePage),
                    Some(page) => {
                        if cold.contains(id.as_str()) {
                            match (&nodes, &mut cursor) {
                                (Some(nodes), Some(cursor)) => match cursor.get(nodes, id.as_str())
                                {
                                    Some(selected) => {
                                        safe = selected.all_paint_sources(|root| {
                                            cache.resource_free(&job.scene, root)
                                        });
                                        crate::render_selected_node_raster_bytes(
                                            selected, job.format, job.scale,
                                        )
                                    }
                                    None => Err(ExportError::NodeNotFoundOnActivePage {
                                        node_id: id.clone(),
                                    }),
                                },
                                _ => Err(ExportError::NoActivePage),
                            }
                        } else {
                            // A font-generation change or LRU eviction after
                            // preflight may turn a formerly hot ID cold. Resolve
                            // only this missing target, never return a false miss.
                            let fallback = SceneSelectionBatch::new(page, [id.as_str()]);
                            match fallback.get(id) {
                                Some(selected) => {
                                    safe = selected.all_paint_sources(|root| {
                                        cache.resource_free(&job.scene, root)
                                    });
                                    crate::render_selected_node_raster_bytes(
                                        &selected, job.format, job.scale,
                                    )
                                }
                                None => Err(ExportError::NodeNotFoundOnActivePage {
                                    node_id: id.clone(),
                                }),
                            }
                        }
                    }
                };
                remember(cache, &job.scene, key, safe, &result);
                result
            };
            if job.reply.send((index, result)).is_err() || job.advance.recv().is_err() {
                break;
            }
        }
    });
}
