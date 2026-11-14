//! Bounded rendezvous queue: one caller drains a FIFO group, then wakes replies.
//! No worker thread, polling loop, detached task, retry or document ownership.
use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

pub(super) struct Limits {
    pub calls: usize,
    pub bytes: usize,
    pub group_calls: usize,
    pub group_bytes: usize,
    pub wait: Duration,
}

enum State<T, R> {
    Pending(Option<T>),
    Claimed,
    Abandoned,
    Complete(Option<R>),
    Canceled,
    Consumed,
}

struct Job<T, R> {
    state: Mutex<State<T, R>>,
    charge: usize,
    body_bytes: usize,
}

struct Inner<T, R> {
    pending: VecDeque<Arc<Job<T, R>>>,
    retained_calls: usize,
    retained_bytes: usize,
    leader: bool,
}

pub(super) struct Queue<T, R> {
    inner: Mutex<Inner<T, R>>,
    ready: Condvar,
    limits: Limits,
}

pub(super) struct Ticket<'a, T, R> {
    queue: &'a Queue<T, R>,
    job: Arc<Job<T, R>>,
    consumed: bool,
}

impl<T, R> Queue<T, R> {
    pub(super) fn new(limits: Limits) -> Self {
        assert!(limits.calls > 0 && limits.group_calls > 0);
        assert!(limits.bytes > 0 && limits.group_bytes > 0);
        Self {
            inner: Mutex::new(Inner {
                pending: VecDeque::new(),
                retained_calls: 0,
                retained_bytes: 0,
                leader: false,
            }),
            ready: Condvar::new(),
            limits,
        }
    }

    /// A full/oversized queue returns the unexecuted input to its caller.
    /// `charge` includes input allocation and the caller's bounded reply reserve.
    pub(super) fn submit(
        &self,
        input: T,
        body_bytes: usize,
        charge: usize,
    ) -> Result<Ticket<'_, T, R>, T> {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if inner.retained_calls >= self.limits.calls
            || charge > self.limits.bytes.saturating_sub(inner.retained_bytes)
            || body_bytes > self.limits.group_bytes
        {
            return Err(input);
        }
        let job = Arc::new(Job {
            state: Mutex::new(State::Pending(Some(input))),
            body_bytes,
            charge,
        });
        inner.retained_calls += 1;
        inner.retained_bytes += charge;
        inner.pending.push_back(Arc::clone(&job));
        self.ready.notify_all();
        Ok(Ticket {
            queue: self,
            job,
            consumed: false,
        })
    }

    #[cfg(test)]
    pub(super) fn retained(&self) -> (usize, usize) {
        let inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        (inner.retained_calls, inner.retained_bytes)
    }
}

impl<T, R> Ticket<'_, T, R> {
    /// Only the oldest waiting caller can lead. The executor receives borrowed
    /// inputs, so panic fallback can answer every claimed request without replay.
    /// It must finish its document commit/rollback before returning or unwinding.
    pub(super) fn execute(
        mut self,
        run: impl FnOnce(&[T]) -> Vec<R>,
        failed: impl Fn(&T) -> R,
    ) -> R {
        let queue = self.queue;
        let mut run = Some(run);
        loop {
            let mut inner = queue.inner.lock().unwrap_or_else(|p| p.into_inner());
            {
                let mut state = self.job.state.lock().unwrap_or_else(|p| p.into_inner());
                if let State::Complete(reply) = &mut *state {
                    let reply = reply.take().expect("one reply consumer");
                    *state = State::Consumed;
                    inner.retained_calls -= 1;
                    inner.retained_bytes -= self.job.charge;
                    self.consumed = true;
                    return reply;
                }
            }
            let first = inner
                .pending
                .front()
                .is_some_and(|job| Arc::ptr_eq(job, &self.job));
            if inner.leader || !first {
                drop(queue.ready.wait(inner).unwrap_or_else(|p| p.into_inner()));
                continue;
            }
            inner.leader = true;
            // One bounded collection window. Incoming notifications never reset
            // its deadline, and a complete group starts without another delay.
            let deadline = Instant::now() + queue.limits.wait;
            while inner.pending.len() < queue.limits.group_calls {
                let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                    break;
                };
                let (next, timeout) = queue
                    .ready
                    .wait_timeout(inner, remaining)
                    .unwrap_or_else(|p| p.into_inner());
                inner = next;
                if timeout.timed_out() {
                    break;
                }
            }
            let mut jobs = Vec::with_capacity(queue.limits.group_calls);
            let mut inputs = Vec::with_capacity(queue.limits.group_calls);
            let mut bytes = 0usize;
            while jobs.len() < queue.limits.group_calls {
                let Some(next) = inner.pending.front() else {
                    break;
                };
                if next.body_bytes > queue.limits.group_bytes.saturating_sub(bytes) {
                    break;
                }
                let next = inner.pending.pop_front().unwrap();
                let mut state = next.state.lock().unwrap_or_else(|p| p.into_inner());
                let State::Pending(input) = &mut *state else {
                    unreachable!("pending FIFO")
                };
                inputs.push(input.take().expect("one input owner"));
                *state = State::Claimed;
                bytes += next.body_bytes;
                drop(state);
                jobs.push(next);
            }
            drop(inner);
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let replies = run.take().expect("one leadership per ticket")(&inputs);
                assert_eq!(
                    replies.len(),
                    inputs.len(),
                    "executor must answer every claimed input"
                );
                replies
            }));
            let replies = outcome.unwrap_or_else(|_| inputs.iter().map(&failed).collect());
            // Release execution pins before any completed caller can receive a
            // reply and submit its next sequential call. Original admissions
            // still cover the network write; abandoned calls are already done.
            drop(inputs);
            let mut inner = queue.inner.lock().unwrap_or_else(|p| p.into_inner());
            for (job, reply) in jobs.into_iter().zip(replies) {
                let mut state = job.state.lock().unwrap_or_else(|p| p.into_inner());
                match &*state {
                    State::Claimed => *state = State::Complete(Some(reply)),
                    State::Abandoned => {
                        // Already claimed work is never replayed or rolled back
                        // merely because its reply waiter disappeared.
                        drop(reply);
                        *state = State::Consumed;
                        inner.retained_calls -= 1;
                        inner.retained_bytes -= job.charge;
                    }
                    _ => unreachable!("one claimed-group completion"),
                }
            }
            inner.leader = false;
            queue.ready.notify_all();
            // Consume this caller's response in the same path as every follower.
        }
    }
}

impl<T, R> Drop for Ticket<'_, T, R> {
    fn drop(&mut self) {
        if self.consumed {
            return;
        }
        let queue = self.queue;
        let mut inner = queue.inner.lock().unwrap_or_else(|p| p.into_inner());
        let mut state = self.job.state.lock().unwrap_or_else(|p| p.into_inner());
        match &*state {
            State::Pending(_) => {
                inner.pending.retain(|job| !Arc::ptr_eq(job, &self.job));
                *state = State::Canceled;
                inner.retained_calls -= 1;
                inner.retained_bytes -= self.job.charge;
                queue.ready.notify_all();
            }
            State::Complete(_) => {
                *state = State::Consumed;
                inner.retained_calls -= 1;
                inner.retained_bytes -= self.job.charge;
            }
            State::Claimed => {
                *state = State::Abandoned;
            }
            State::Abandoned | State::Canceled | State::Consumed => {}
        }
    }
}

#[cfg(test)]
#[path = "managed_group_queue_tests.rs"]
mod tests;
