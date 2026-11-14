//! Detached HTTP scene handles and post-authority FIFO renderer admission.
use crate::export::{shared_raster_worker, ExportError, RasterAdmission, WorkCancellation};
use op_editor_ui::layout_scene::LayoutScene;
use op_pen_loader::{CachedExportScene, ExportSceneUnavailable};
use std::{
    ops::Deref,
    sync::{Arc, OnceLock},
};

pub(super) struct Captured {
    source: CachedExportScene,
    ready: OnceLock<Result<Arc<LayoutScene>, ExportSceneUnavailable>>,
}
#[derive(Clone)]
pub(super) enum Scene {
    Immediate(Arc<LayoutScene>),
    Shared(Arc<Captured>),
}
impl Scene {
    pub(super) fn captured(source: CachedExportScene) -> Self {
        Self::Shared(Arc::new(Captured {
            source,
            ready: OnceLock::new(),
        }))
    }
    pub(super) fn ready(&self) -> Result<(), ExportError> {
        match self {
            Self::Immediate(_) => Ok(()),
            Self::Shared(value) => {
                if value.ready.get().is_none() && !super::super::http_export::executing() {
                    return Err(ExportError::RenderWorkerUnavailable);
                }
                value
                    .ready
                    .get_or_init(|| value.source.resolve())
                    .as_ref()
                    .map(|_| ())
                    .map_err(|_| ExportError::SceneBuildUnavailable)
            }
        }
    }
}
impl Deref for Scene {
    type Target = Arc<LayoutScene>;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Immediate(scene) => scene,
            Self::Shared(value) => value
                .ready
                .get()
                .and_then(|value| value.as_ref().ok())
                .expect(
                    "deferred scene access requires successful ready() after document dispatch",
                ),
        }
    }
}
pub(super) struct Queued {
    cancel: WorkCancellation,
    permit: OnceLock<Result<Arc<RasterAdmission>, ExportError>>,
}
impl Queued {
    pub(super) fn new(cancel: WorkCancellation) -> Self {
        Self {
            cancel,
            permit: OnceLock::new(),
        }
    }
    pub(super) fn get(&self) -> Result<&Arc<RasterAdmission>, ExportError> {
        if let Some(permit) = self.permit.get() {
            return permit.as_ref().map_err(Clone::clone);
        }
        if !super::super::http_export::executing() {
            return Err(ExportError::RenderWorkerUnavailable);
        }
        self.permit
            .get_or_init(|| {
                let permit = shared_raster_worker()?
                    .acquire_queued(&self.cancel)
                    .map(Arc::new)?;
                super::super::raster_response::retain(&permit);
                Ok(permit)
            })
            .as_ref()
            .map_err(Clone::clone)
    }
}
