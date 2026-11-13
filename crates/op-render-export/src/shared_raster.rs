//! A shared, bounded raster worker for headless request handlers.
//!
//! Acquire admission before deriving a scene. The worker owns its renderer on
//! its dedicated thread; no Skia canvas or non-Send NativeBackend crosses the
//! channel. Raw canvas callbacks remain synchronous on their calling thread.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc, Arc, Mutex, OnceLock,
};

use op_editor_ui::layout_scene::LayoutScene;

use crate::{ExportError, RasterFormat};

const MAX_ADMITTED: usize = 2;

/// Resolved page/node identity; the scene is shared without a document clone.
#[derive(Debug, Clone)]
pub enum SharedRasterTarget {
    Page { page_id: String },
    Node { page_id: String, node_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RasterWorkerStats {
    pub admitted: usize,
    pub completed_jobs: usize,
    pub backend_initializations: usize,
}

#[derive(Default)]
struct Counters {
    admitted: AtomicUsize,
    completed: AtomicUsize,
    backend_initializations: AtomicUsize,
}

struct Job {
    scene: Arc<LayoutScene>,
    target: SharedRasterTarget,
    format: RasterFormat,
    scale: f32,
    reply: mpsc::SyncSender<Result<Vec<u8>, ExportError>>,
}

struct WorkerState {
    sender: mpsc::SyncSender<Job>,
    counters: Arc<Counters>,
}

/// Cloneable handle to one render thread, not one renderer per request.
#[derive(Clone)]
pub struct SharedRasterWorker {
    state: Arc<WorkerState>,
}

/// Request-owned capacity. Keep this alive through response construction.
/// Dropping it releases admission; a rejected request need not derive a scene.
pub struct RasterAdmission {
    state: Arc<WorkerState>,
    submission: Mutex<()>,
}

impl SharedRasterWorker {
    pub fn new() -> Result<Self, ExportError> {
        // One queued job plus one rendering. Replies have zero capacity, so
        // the worker never accumulates completed encoded images in a queue.
        let (sender, receiver) = mpsc::sync_channel::<Job>(1);
        let counters = Arc::new(Counters::default());
        let worker_counters = Arc::clone(&counters);
        std::thread::Builder::new()
            .name("op-shared-raster".into())
            .spawn(move || {
                let mut initialized = false;
                for job in receiver {
                    if !initialized {
                        crate::scene_painter::initialize_backend();
                        worker_counters
                            .backend_initializations
                            .fetch_add(1, Ordering::Relaxed);
                        initialized = true;
                    }
                    let result = render_job(&job);
                    worker_counters.completed.fetch_add(1, Ordering::Relaxed);
                    let _ = job.reply.send(result);
                }
            })
            .map_err(|error| ExportError::RenderWorkerStart {
                message: error.to_string(),
            })?;
        Ok(Self {
            state: Arc::new(WorkerState { sender, counters }),
        })
    }

    /// Fail fast before layout derivation, scene cloning, or raster allocation.
    pub fn try_acquire(&self) -> Result<RasterAdmission, ExportError> {
        self.state
            .counters
            .admitted
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_ADMITTED).then_some(count + 1)
            })
            .map_err(|_| ExportError::RenderWorkerBusy {
                max_admitted: MAX_ADMITTED,
            })?;
        Ok(RasterAdmission {
            state: Arc::clone(&self.state),
            submission: Mutex::new(()),
        })
    }

    pub fn stats(&self) -> RasterWorkerStats {
        RasterWorkerStats {
            admitted: self.state.counters.admitted.load(Ordering::Acquire),
            completed_jobs: self.state.counters.completed.load(Ordering::Relaxed),
            backend_initializations: self
                .state
                .counters
                .backend_initializations
                .load(Ordering::Relaxed),
        }
    }
}

impl RasterAdmission {
    /// Render a shared immutable scene. Calls using the same admission are
    /// serialized, so sharing a tool cannot bypass the two-request bound.
    pub fn render(
        &self,
        scene: Arc<LayoutScene>,
        target: SharedRasterTarget,
        format: RasterFormat,
        scale: f32,
    ) -> Result<Vec<u8>, ExportError> {
        let _submission = self
            .submission
            .lock()
            .map_err(|_| ExportError::RenderWorkerUnavailable)?;
        let (reply, receive) = mpsc::sync_channel(0);
        self.state
            .sender
            .send(Job {
                scene,
                target,
                format,
                scale,
                reply,
            })
            .map_err(|_| ExportError::RenderWorkerUnavailable)?;
        receive
            .recv()
            .map_err(|_| ExportError::RenderWorkerUnavailable)?
    }
}

impl Drop for RasterAdmission {
    fn drop(&mut self) {
        self.state.counters.admitted.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Process-wide headless renderer. Its backend is constructed lazily on the
/// worker thread, only when a raster job is actually submitted.
pub fn shared_raster_worker() -> Result<&'static SharedRasterWorker, ExportError> {
    static WORKER: OnceLock<Result<SharedRasterWorker, ExportError>> = OnceLock::new();
    WORKER
        .get_or_init(SharedRasterWorker::new)
        .as_ref()
        .map_err(Clone::clone)
}

fn render_job(job: &Job) -> Result<Vec<u8>, ExportError> {
    let page_id = match &job.target {
        SharedRasterTarget::Page { page_id } | SharedRasterTarget::Node { page_id, .. } => page_id,
    };
    let page = job
        .scene
        .pages
        .iter()
        .find(|page| &page.id == page_id)
        .ok_or_else(|| ExportError::PageNotFound {
            page_id: page_id.clone(),
        })?;
    match &job.target {
        SharedRasterTarget::Page { .. } => {
            crate::render_page_raster_bytes(page, job.format, job.scale)
        }
        SharedRasterTarget::Node { node_id, .. } => {
            crate::render_node_on_page_raster_bytes(page, node_id, job.format, job.scale)
        }
    }
}

#[cfg(test)]
#[path = "shared_raster_tests.rs"]
mod tests;
