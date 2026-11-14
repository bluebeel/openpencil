//! Lazy, bounded connection workers. SSE can occupy a worker for its whole
//! lifetime; short MCP requests reuse idle workers instead of spawning a new
//! OS thread for every operation. The accept loops retain connection admission.

use std::{
    fmt,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{mpsc, Arc, Mutex},
    thread,
};

type Job = Box<dyn FnOnce() + Send + 'static>;

#[derive(Debug)]
pub(crate) enum PoolError {
    Spawn(std::io::Error),
    Full,
    Closed,
}

impl fmt::Display for PoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(error) => write!(f, "start connection worker: {error}"),
            Self::Full => f.write_str("connection worker queue is full"),
            Self::Closed => f.write_str("connection worker queue is closed"),
        }
    }
}

impl std::error::Error for PoolError {}

pub(crate) struct ConnectionPool {
    sender: mpsc::SyncSender<Job>,
    receiver: Arc<Mutex<mpsc::Receiver<Job>>>,
    workers: usize,
    max_workers: usize,
    name: &'static str,
    stack_size: Option<usize>,
}

impl ConnectionPool {
    pub fn new(max_workers: usize, name: &'static str) -> Self {
        let max_workers = max_workers.max(1);
        let (sender, receiver) = mpsc::sync_channel(max_workers);
        Self {
            sender,
            receiver: Arc::new(Mutex::new(receiver)),
            workers: 0,
            max_workers,
            name,
            stack_size: None,
        }
    }

    /// Keep endpoint-specific recursion headroom when reusing its workers.
    pub fn with_stack_size(mut self, bytes: usize) -> Self {
        self.stack_size = Some(bytes);
        self
    }

    /// `connections` includes this accepted socket and long-lived SSE streams.
    /// Grow only to observed demand. A rejected job is dropped here, releasing
    /// its socket and the captured connection-count guard on every error path.
    pub fn submit(&mut self, connections: usize, job: Job) -> Result<(), PoolError> {
        let desired = connections.clamp(1, self.max_workers);
        if self.workers < desired {
            let receiver = Arc::clone(&self.receiver);
            let mut builder =
                thread::Builder::new().name(format!("{}-{}", self.name, self.workers));
            if let Some(bytes) = self.stack_size {
                builder = builder.stack_size(bytes);
            }
            builder
                .spawn(move || loop {
                    let next = {
                        let receiver = receiver.lock().unwrap_or_else(|p| p.into_inner());
                        receiver.recv()
                    };
                    let Ok(job) = next else { break };
                    // One bad request must release its admission guard and
                    // socket without permanently shrinking worker capacity.
                    let _ = catch_unwind(AssertUnwindSafe(job));
                })
                .map_err(PoolError::Spawn)?;
            self.workers += 1;
        }
        self.sender.try_send(job).map_err(|error| match error {
            mpsc::TrySendError::Full(_) => PoolError::Full,
            mpsc::TrySendError::Disconnected(_) => PoolError::Closed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashSet,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Barrier,
        },
        time::Duration,
    };

    #[test]
    fn sequential_requests_reuse_one_worker() {
        let mut pool = ConnectionPool::new(64, "pool-reuse-test").with_stack_size(16 * 1024 * 1024);
        let (done, responses) = mpsc::channel();
        let mut threads = HashSet::new();
        for _ in 0..1000 {
            let done = done.clone();
            pool.submit(
                1,
                Box::new(move || {
                    done.send(thread::current().id()).unwrap();
                }),
            )
            .unwrap();
            threads.insert(responses.recv_timeout(Duration::from_secs(5)).unwrap());
        }
        assert_eq!(pool.workers, 1);
        assert_eq!(threads.len(), 1);
    }

    #[test]
    fn occupied_streams_keep_parallel_capacity_and_the_queue_is_bounded() {
        let mut pool = ConnectionPool::new(2, "pool-bound-test");
        let barrier = Arc::new(Barrier::new(3));
        let (started, ready) = mpsc::channel();
        let (done, responses) = mpsc::channel();
        for _ in 0..2 {
            let barrier = Arc::clone(&barrier);
            let started = started.clone();
            pool.submit(
                2,
                Box::new(move || {
                    started.send(()).unwrap();
                    barrier.wait();
                }),
            )
            .unwrap();
        }
        for _ in 0..2 {
            ready.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        for i in 0..2 {
            let done = done.clone();
            pool.submit(
                2,
                Box::new(move || {
                    done.send(i).unwrap();
                }),
            )
            .unwrap();
        }
        assert!(matches!(
            pool.submit(2, Box::new(|| {})),
            Err(PoolError::Full)
        ));
        assert_eq!(pool.workers, 2);
        barrier.wait();
        let mut received = [
            responses.recv_timeout(Duration::from_secs(5)).unwrap(),
            responses.recv_timeout(Duration::from_secs(5)).unwrap(),
        ];
        received.sort();
        assert_eq!(received, [0, 1]);
    }

    #[test]
    fn panicking_jobs_release_resources_and_leave_worker_usable() {
        struct Lease(Arc<AtomicUsize>);
        impl Drop for Lease {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::AcqRel);
            }
        }
        let active = Arc::new(AtomicUsize::new(2));
        let mut pool = ConnectionPool::new(1, "pool-panic-test");
        let lease = Lease(Arc::clone(&active));
        let (started, ready) = mpsc::channel();
        pool.submit(
            1,
            Box::new(move || {
                let _lease = lease;
                started.send(()).unwrap();
                panic!("request panic");
            }),
        )
        .unwrap();
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        let lease = Lease(Arc::clone(&active));
        let (done, finished) = mpsc::channel();
        pool.submit(
            1,
            Box::new(move || {
                drop(lease);
                done.send(()).unwrap();
            }),
        )
        .unwrap();
        finished.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(active.load(Ordering::Acquire), 0);
        assert_eq!(pool.workers, 1);
    }
}
