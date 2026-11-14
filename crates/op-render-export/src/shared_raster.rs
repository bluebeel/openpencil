//! A shared, bounded raster worker for headless request handlers.
//!
//! Acquire admission before deriving a scene. The worker owns its renderer on
//! its dedicated thread; no Skia canvas or non-Send NativeBackend crosses the
//! channel. Raw canvas callbacks remain synchronous on their calling thread.

use std::cell::Cell;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc, Arc, Mutex, OnceLock,
};

use op_editor_ui::layout_scene::LayoutScene;

use crate::{ExportError, RasterFileTarget, RasterFormat, RasterSampling};

use crate::shared_work::{
    shared_work_gate, SharedWorkGate, SharedWorkLease, MAX_RENDER_ADMISSIONS,
};

const MAX_ADMITTED: usize = MAX_RENDER_ADMISSIONS;

thread_local! { static CONSUMING_BATCH: Cell<bool> = const { Cell::new(false) }; }
fn check_consumer_reentry() -> Result<(), ExportError> {
    if CONSUMING_BATCH.with(Cell::get) {
        Err(ExportError::RenderConsumerReentry)
    } else {
        Ok(())
    }
}
fn consume_batch_item<R>(consume: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            CONSUMING_BATCH.with(|flag| flag.set(self.0));
        }
    }
    let _restore = Restore(CONSUMING_BATCH.with(|flag| flag.replace(true)));
    consume()
}

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

struct RasterJob {
    scene: Arc<LayoutScene>,
    target: SharedRasterTarget,
    format: RasterFormat,
    scale: f32,
    sampling: RasterSampling,
    reply: mpsc::SyncSender<Result<Vec<u8>, ExportError>>,
}

struct FileJob {
    scene: Arc<LayoutScene>,
    targets: Vec<RasterFileTarget>,
    format: RasterFormat,
    scale: f32,
    reply: mpsc::SyncSender<Vec<Result<(), ExportError>>>,
}

struct NodeBatchJob {
    scene: Arc<LayoutScene>,
    node_ids: Vec<String>,
    format: RasterFormat,
    scale: f32,
    reply: mpsc::SyncSender<(usize, Result<Vec<u8>, ExportError>)>,
    advance: mpsc::Receiver<()>,
}

enum Job {
    Raster(RasterJob),
    Files(FileJob),
    Nodes(NodeBatchJob),
}

struct WorkerState {
    sender: mpsc::SyncSender<Job>,
    counters: Arc<Counters>,
    work_gate: SharedWorkGate,
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
    _work_lease: SharedWorkLease,
}

impl SharedRasterWorker {
    pub fn new() -> Result<Self, ExportError> {
        Self::with_work_gate(SharedWorkGate::new())
    }

    pub(crate) fn with_work_gate(work_gate: SharedWorkGate) -> Result<Self, ExportError> {
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
                    match job {
                        Job::Raster(job) => {
                            let result = render_job(&job);
                            worker_counters.completed.fetch_add(1, Ordering::Relaxed);
                            let _ = job.reply.send(result);
                        }
                        Job::Nodes(job) => {
                            render_node_batch_job(job);
                            worker_counters.completed.fetch_add(1, Ordering::Relaxed);
                        }
                        Job::Files(job) => {
                            let results = crate::export_scene_files(
                                &job.scene,
                                &job.targets,
                                job.format,
                                job.scale,
                            );
                            worker_counters.completed.fetch_add(1, Ordering::Relaxed);
                            let _ = job.reply.send(results);
                        }
                    }
                }
            })
            .map_err(|error| ExportError::RenderWorkerStart {
                message: error.to_string(),
            })?;
        Ok(Self {
            state: Arc::new(WorkerState {
                sender,
                counters,
                work_gate,
            }),
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
        let Some(work_lease) = self.state.work_gate.try_render() else {
            self.state.counters.admitted.fetch_sub(1, Ordering::AcqRel);
            return Err(ExportError::SharedWorkBusy);
        };
        Ok(RasterAdmission {
            state: Arc::clone(&self.state),
            submission: Mutex::new(()),
            _work_lease: work_lease,
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
        self.render_with_sampling(scene, target, format, scale, RasterSampling::Native)
    }

    pub fn render_with_sampling(
        &self,
        scene: Arc<LayoutScene>,
        target: SharedRasterTarget,
        format: RasterFormat,
        scale: f32,
        sampling: RasterSampling,
    ) -> Result<Vec<u8>, ExportError> {
        check_consumer_reentry()?;
        let _submission = self
            .submission
            .lock()
            .map_err(|_| ExportError::RenderWorkerUnavailable)?;
        let (reply, receive) = mpsc::sync_channel(0);
        self.state
            .sender
            .send(Job::Raster(RasterJob {
                scene,
                target,
                format,
                scale,
                sampling,
                reply,
            }))
            .map_err(|_| ExportError::RenderWorkerUnavailable)?;
        receive
            .recv()
            .map_err(|_| ExportError::RenderWorkerUnavailable)?
    }

    /// Resolve a byte-export batch once on the worker, then hand off only one
    /// encoded result at a time. The acknowledgment follows consumption, so
    /// the worker cannot allocate the next output while this one is encoded.
    /// A panicking/dropped consumer disconnects the acknowledgment channel and
    /// releases the worker without retaining the remaining output payloads.
    pub fn render_active_nodes(
        &self,
        scene: Arc<LayoutScene>,
        node_ids: Vec<String>,
        format: RasterFormat,
        scale: f32,
        mut consume: impl FnMut(usize, Result<Vec<u8>, ExportError>),
    ) -> Result<(), ExportError> {
        self.render_active_nodes_until(scene, node_ids, format, scale, |index, result| {
            consume(index, result);
            true
        })
    }

    /// A false callback result disconnects the ACK before another target is
    /// rendered. Transport writers use this for broken pipes/backpressure.
    pub fn render_active_nodes_until(
        &self,
        scene: Arc<LayoutScene>,
        node_ids: Vec<String>,
        format: RasterFormat,
        scale: f32,
        mut consume: impl FnMut(usize, Result<Vec<u8>, ExportError>) -> bool,
    ) -> Result<(), ExportError> {
        if node_ids.is_empty() {
            return Ok(());
        }
        check_consumer_reentry()?;
        let _submission = self
            .submission
            .lock()
            .map_err(|_| ExportError::RenderWorkerUnavailable)?;
        let count = node_ids.len();
        let (reply, receive) = mpsc::sync_channel(0);
        let (advance, next) = mpsc::sync_channel(0);
        self.state
            .sender
            .send(Job::Nodes(NodeBatchJob {
                scene,
                node_ids,
                format,
                scale,
                reply,
                advance: next,
            }))
            .map_err(|_| ExportError::RenderWorkerUnavailable)?;
        for expected in 0..count {
            let (index, result) = receive
                .recv()
                .map_err(|_| ExportError::RenderWorkerUnavailable)?;
            if index != expected {
                return Err(ExportError::RenderWorkerUnavailable);
            }
            if !consume_batch_item(|| consume(index, result)) {
                return Ok(());
            }
            advance
                .send(())
                .map_err(|_| ExportError::RenderWorkerUnavailable)?;
        }
        Ok(())
    }

    /// Stream a whole file batch on the same renderer thread as byte exports.
    /// No editor state or encoded file buffers cross the request channel.
    pub fn export_files(
        &self,
        scene: Arc<LayoutScene>,
        targets: Vec<RasterFileTarget>,
        format: RasterFormat,
        scale: f32,
    ) -> Result<Vec<Result<(), ExportError>>, ExportError> {
        check_consumer_reentry()?;
        let _submission = self
            .submission
            .lock()
            .map_err(|_| ExportError::RenderWorkerUnavailable)?;
        let (reply, receive) = mpsc::sync_channel(0);
        self.state
            .sender
            .send(Job::Files(FileJob {
                scene,
                targets,
                format,
                scale,
                reply,
            }))
            .map_err(|_| ExportError::RenderWorkerUnavailable)?;
        receive
            .recv()
            .map_err(|_| ExportError::RenderWorkerUnavailable)
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
        .get_or_init(|| SharedRasterWorker::with_work_gate(shared_work_gate().clone()))
        .as_ref()
        .map_err(Clone::clone)
}

fn render_job(job: &RasterJob) -> Result<Vec<u8>, ExportError> {
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
            crate::render_page_raster_bytes_with_sampling(page, job.format, job.scale, job.sampling)
        }
        SharedRasterTarget::Node { node_id, .. } => {
            crate::render_node_on_page_raster_bytes_with_sampling(
                page,
                node_id,
                job.format,
                job.scale,
                job.sampling,
            )
        }
    }
}

fn render_node_batch_job(job: NodeBatchJob) {
    let page = job.scene.active_page();
    let nodes = page
        .map(|page| crate::SceneSelectionBatch::new(page, job.node_ids.iter().map(String::as_str)));
    for (index, id) in job.node_ids.iter().enumerate() {
        let result = match &nodes {
            None => Err(ExportError::NoActivePage),
            Some(nodes) => match nodes.get(id.as_str()) {
                None => Err(ExportError::NodeNotFoundOnActivePage {
                    node_id: id.clone(),
                }),
                Some(selected) => {
                    crate::render_selected_node_raster_bytes(&selected, job.format, job.scale)
                }
            },
        };
        if job.reply.send((index, result)).is_err() || job.advance.recv().is_err() {
            break;
        }
    }
}

#[cfg(test)]
#[path = "shared_raster_tests.rs"]
mod tests;
