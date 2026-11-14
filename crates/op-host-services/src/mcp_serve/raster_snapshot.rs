//! Request-scoped raster admission, taken before layout snapshots are built.

use std::{ops::Deref, sync::Arc};

use op_editor_ui::layout_scene::LayoutScene;

use crate::export::{
    shared_raster_worker, ExportError, RasterAdmission, RasterFormat, RasterSampling,
    SharedRasterTarget,
};

#[derive(Clone)]
pub(super) struct RasterSnapshot {
    scene: Arc<LayoutScene>,
    admission: Option<Result<Arc<RasterAdmission>, ExportError>>,
}

impl RasterSnapshot {
    /// Legacy/discovery construction claims no headless request capacity.
    pub fn from_scene(scene: LayoutScene) -> Self {
        Self {
            scene: Arc::new(scene),
            admission: None,
        }
    }

    /// The factory is never called when the shared renderer is saturated.
    pub fn admitted(derive: impl FnOnce() -> LayoutScene) -> Self {
        Self::from_admission(
            shared_raster_worker().and_then(|worker| worker.try_acquire()),
            derive,
        )
    }

    fn from_admission(
        admission: Result<RasterAdmission, ExportError>,
        derive: impl FnOnce() -> LayoutScene,
    ) -> Self {
        match admission {
            Ok(admission) => {
                let admission = Arc::new(admission);
                super::raster_response::retain(&admission);
                Self {
                    scene: Arc::new(derive()),
                    admission: Some(Ok(admission)),
                }
            }
            Err(error) => Self {
                scene: Arc::new(LayoutScene {
                    pages: Vec::new(),
                    active_page_index: 0,
                }),
                admission: Some(Err(error)),
            },
        }
    }

    pub fn ready(&self) -> Result<(), ExportError> {
        match &self.admission {
            Some(Err(error)) => Err(error.clone()),
            _ => Ok(()),
        }
    }

    pub fn render(
        &self,
        target: SharedRasterTarget,
        format: RasterFormat,
        scale: f32,
    ) -> Result<Vec<u8>, ExportError> {
        self.render_with_sampling(target, format, scale, RasterSampling::Native)
    }

    pub fn render_with_sampling(
        &self,
        target: SharedRasterTarget,
        format: RasterFormat,
        scale: f32,
        sampling: RasterSampling,
    ) -> Result<Vec<u8>, ExportError> {
        let admission = match &self.admission {
            Some(Ok(admission)) => admission.as_ref(),
            Some(Err(error)) => return Err(error.clone()),
            None => {
                // Preserve synchronous public-factory/discovery behavior.
                // Only request-specific constructors enter the shared worker.
                let page_id = match &target {
                    SharedRasterTarget::Page { page_id }
                    | SharedRasterTarget::Node { page_id, .. } => page_id,
                };
                let page = self
                    .scene
                    .pages
                    .iter()
                    .find(|page| &page.id == page_id)
                    .ok_or_else(|| ExportError::PageNotFound {
                        page_id: page_id.clone(),
                    })?;
                return match target {
                    SharedRasterTarget::Page { .. } => {
                        crate::export::render_page_raster_bytes_with_sampling(
                            page, format, scale, sampling,
                        )
                    }
                    SharedRasterTarget::Node { node_id, .. } => {
                        crate::export::render_node_on_page_raster_bytes_with_sampling(
                            page, &node_id, format, scale, sampling,
                        )
                    }
                };
            }
        };
        admission.render_with_sampling(Arc::clone(&self.scene), target, format, scale, sampling)
    }

    pub fn export_files(
        &self,
        targets: Vec<crate::export::RasterFileTarget>,
        format: RasterFormat,
        scale: f32,
    ) -> Result<Vec<Result<(), ExportError>>, ExportError> {
        match &self.admission {
            Some(Ok(admission)) => {
                admission.export_files(Arc::clone(&self.scene), targets, format, scale)
            }
            Some(Err(error)) => Err(error.clone()),
            None => Ok(crate::export::export_scene_files(
                &self.scene,
                &targets,
                format,
                scale,
            )),
        }
    }

    /// One lookup per immutable request, with ordered per-ID handoff. Capacity
    /// remains held through every consumer callback and no whole-scene copy is
    /// made. Infrastructure failure is reported for every unconsumed entry.
    pub fn render_active_nodes(
        &self,
        node_ids: &[String],
        format: RasterFormat,
        scale: f32,
        mut consume: impl FnMut(&str, Result<Vec<u8>, ExportError>),
    ) {
        self.render_active_nodes_until(node_ids, format, scale, |id, result| {
            consume(id, result);
            true
        });
    }

    pub fn render_active_nodes_until(
        &self,
        node_ids: &[String],
        format: RasterFormat,
        scale: f32,
        mut consume: impl FnMut(&str, Result<Vec<u8>, ExportError>) -> bool,
    ) {
        if let Err(error) = self.ready() {
            for id in node_ids {
                if !consume(id, Err(error.clone())) {
                    break;
                }
            }
            return;
        }
        let Some(page) = self.active_page() else {
            for id in node_ids {
                if !consume(id, Err(ExportError::NoActivePage)) {
                    break;
                }
            }
            return;
        };
        if let Some(Ok(admission)) = &self.admission {
            let mut consumed = 0;
            let result = admission.render_active_nodes_until(
                Arc::clone(&self.scene),
                node_ids.to_vec(),
                format,
                scale,
                |index, result| {
                    consumed = index + 1;
                    consume(&node_ids[index], result)
                },
            );
            if let Err(error) = result {
                for id in &node_ids[consumed..] {
                    if !consume(id, Err(error.clone())) {
                        break;
                    }
                }
            }
        } else {
            let nodes =
                crate::export::SceneSelectionBatch::new(page, node_ids.iter().map(String::as_str));
            for id in node_ids {
                let result = nodes
                    .get(id.as_str())
                    .ok_or_else(|| ExportError::NodeNotFoundOnActivePage {
                        node_id: id.clone(),
                    })
                    .and_then(|selected| {
                        crate::export::render_selected_node_raster_bytes(&selected, format, scale)
                    });
                if !consume(id, result) {
                    break;
                }
            }
        }
    }

    pub fn render_active_node(
        &self,
        node_id: &str,
        format: RasterFormat,
        scale: f32,
    ) -> Result<Vec<u8>, ExportError> {
        self.ready()?;
        let page = self.active_page().ok_or(ExportError::NoActivePage)?;
        // Preserve the legacy active-page error variant before queuing.
        if page.find(node_id).is_none() {
            return Err(ExportError::NodeNotFoundOnActivePage {
                node_id: node_id.to_string(),
            });
        }
        self.render(
            SharedRasterTarget::Node {
                page_id: page.id.clone(),
                node_id: node_id.to_string(),
            },
            format,
            scale,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::SharedRasterWorker;
    use std::cell::Cell;

    fn empty_scene() -> LayoutScene {
        LayoutScene {
            pages: Vec::new(),
            active_page_index: 0,
        }
    }

    #[test]
    fn saturated_renderer_rejects_before_scene_derivation() {
        let worker = SharedRasterWorker::new().unwrap();
        let _first = worker.try_acquire().unwrap();
        let _second = worker.try_acquire().unwrap();
        let called = Cell::new(false);
        let snapshot = RasterSnapshot::from_admission(worker.try_acquire(), || {
            called.set(true);
            empty_scene()
        });
        assert!(
            !called.get(),
            "busy requests must not derive or clone a scene"
        );
        assert_eq!(
            snapshot.ready(),
            Err(ExportError::RenderWorkerBusy { max_admitted: 2 })
        );
    }

    #[test]
    fn lean_delegates_share_scene_and_request_admission() {
        let worker = SharedRasterWorker::new().unwrap();
        let snapshot = RasterSnapshot::from_admission(worker.try_acquire(), empty_scene);
        let delegate = snapshot.clone();
        assert!(Arc::ptr_eq(&snapshot.scene, &delegate.scene));
        assert_eq!(worker.stats().admitted, 1);
        drop(snapshot);
        assert_eq!(worker.stats().admitted, 1);
        drop(delegate);
        assert_eq!(worker.stats().admitted, 0);
    }

    #[test]
    fn transport_keeps_admission_after_snapshot_drops() {
        let worker = SharedRasterWorker::new().unwrap();
        let response = super::super::raster_response::RasterResponseScope::new();
        let snapshot = RasterSnapshot::from_admission(worker.try_acquire(), empty_scene);
        drop(snapshot);
        assert_eq!(worker.stats().admitted, 1);
        drop(response);
        assert_eq!(worker.stats().admitted, 0);
    }
}

impl Deref for RasterSnapshot {
    type Target = LayoutScene;

    fn deref(&self) -> &Self::Target {
        &self.scene
    }
}
