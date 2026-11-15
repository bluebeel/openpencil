//! Bounded rendezvous queue: one caller drains a FIFO group, then wakes replies.
//! Compatibility callers wait synchronously; owned TCP callers use notified
//! futures and bounded cohort execution. No retry or document ownership.
use std::{
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

#[path = "managed_group_pending.rs"]
mod pending;
use pending::Pending;

pub(crate) struct Limits {
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
    ready: Condvar,
    notified: tokio::sync::Notify,
}

struct Inner<T, R> {
    pending: Pending<Arc<Job<T, R>>>,
    pending_body_bytes: usize,
    retained_calls: usize,
    retained_bytes: usize,
    leader: bool,
}

pub(crate) struct Queue<T, R> {
    inner: Arc<Mutex<Inner<T, R>>>,
    collect: Arc<Condvar>,
    collected: Arc<tokio::sync::Notify>,
    limits: Arc<Limits>,
}

pub(crate) struct Ticket<T, R> {
    queue: Queue<T, R>,
    job: Arc<Job<T, R>>,
    consumed: bool,
    // Valid only while Pending. Claim/cancel changes that state while holding
    // the queue lock, so a detached ticket never accesses a reused slot.
    pending_slot: usize,
}

impl<T, R> Queue<T, R> {
    pub(crate) fn new(limits: Limits) -> Self {
        assert!(limits.calls > 0 && limits.group_calls > 0);
        assert!(limits.bytes > 0 && limits.group_bytes > 0);
        Self {
            inner: Arc::new(Mutex::new(Inner {
                pending: Pending::new(),
                pending_body_bytes: 0,
                retained_calls: 0,
                retained_bytes: 0,
                leader: false,
            })),
            collect: Arc::new(Condvar::new()),
            collected: Arc::new(tokio::sync::Notify::new()),
            limits: Arc::new(limits),
        }
    }

    /// A full/oversized queue returns the unexecuted input to its caller.
    /// `charge` includes input allocation and the caller's bounded reply reserve.
    pub(crate) fn submit(
        &self,
        input: T,
        body_bytes: usize,
        charge: usize,
    ) -> Result<Ticket<T, R>, T> {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if charge < body_bytes
            || inner.retained_calls >= self.limits.calls
            || charge > self.limits.bytes.saturating_sub(inner.retained_bytes)
            || body_bytes > self.limits.group_bytes
        {
            return Err(input);
        }
        let job = Arc::new(Job {
            state: Mutex::new(State::Pending(Some(input))),
            body_bytes,
            charge,
            ready: Condvar::new(),
            notified: tokio::sync::Notify::new(),
        });
        inner.retained_calls += 1;
        inner.retained_bytes += charge;
        let pending_slot = inner.pending.push_back(Arc::clone(&job));
        inner.pending_body_bytes += body_bytes;
        if inner.leader {
            self.collect.notify_one();
            self.collected.notify_one();
        } else if let Some(head) = inner.pending.front() {
            head.ready.notify_one();
            head.notified.notify_one();
        }
        Ok(Ticket {
            queue: self.clone(),
            job,
            consumed: false,
            pending_slot,
        })
    }

    #[cfg(test)]
    pub(crate) fn retained(&self) -> (usize, usize) {
        let inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        (inner.retained_calls, inner.retained_bytes)
    }
}

impl<T, R> Ticket<T, R> {
    /// Only the oldest waiting caller can lead. The executor receives borrowed
    /// inputs, so panic fallback can answer every claimed request without replay.
    /// It must finish its document commit/rollback before returning or unwinding.
    pub(crate) fn execute(
        mut self,
        run: impl FnOnce(&[T]) -> Vec<R>,
        failed: impl Fn(&T) -> R,
    ) -> R {
        let queue = &self.queue;
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
                drop(
                    self.job
                        .ready
                        .wait(inner)
                        .unwrap_or_else(|p| p.into_inner()),
                );
                continue;
            }
            inner.leader = true;
            // One bounded collection window. Incoming notifications never reset
            // its deadline, and a complete group starts without another delay.
            let deadline = Instant::now() + queue.limits.wait;
            while inner.pending.len() < queue.limits.group_calls
                && inner.pending_body_bytes < queue.limits.group_bytes
            {
                let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                    break;
                };
                let (next, timeout) = queue
                    .collect
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
                inner.pending_body_bytes -= next.body_bytes;
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
            let mut abandoned = Vec::new();
            for (job, reply) in jobs.into_iter().zip(replies) {
                let mut state = job.state.lock().unwrap_or_else(|p| p.into_inner());
                match &*state {
                    State::Claimed => {
                        *state = State::Complete(Some(reply));
                        job.ready.notify_one();
                        job.notified.notify_one();
                    }
                    State::Abandoned => {
                        // Already claimed work is never replayed or rolled back
                        // merely because its reply waiter disappeared.
                        *state = State::Consumed;
                        abandoned.push((reply, job.charge));
                    }
                    _ => unreachable!("one claimed-group completion"),
                }
            }
            inner.leader = false;
            if let Some(head) = inner.pending.front() {
                head.ready.notify_one();
                head.notified.notify_one();
            }
            drop(inner);
            queue.retire_abandoned(abandoned);
            // Consume this caller's response in the same path as every follower.
        }
    }
}

/// Keep accounting until payload destruction has finished outside queue locks.
struct RetiredCredits<'a, T, R> {
    queue: &'a Queue<T, R>,
    calls: usize,
    bytes: usize,
}
impl<T, R> Drop for RetiredCredits<'_, T, R> {
    fn drop(&mut self) {
        let mut inner = self.queue.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner.retained_calls -= self.calls;
        inner.retained_bytes -= self.bytes;
    }
}
impl<T, R> Queue<T, R> {
    fn retire_abandoned(&self, replies: Vec<(R, usize)>) {
        if replies.is_empty() {
            return;
        }
        let _credits = RetiredCredits {
            queue: self,
            calls: replies.len(),
            bytes: replies.iter().map(|(_, bytes)| *bytes).sum(),
        };
        for (reply, _) in replies {
            drop(reply);
        }
    }
}
impl<T, R> Drop for Ticket<T, R> {
    fn drop(&mut self) {
        if self.consumed {
            return;
        }
        let queue = &self.queue;
        let mut inner = queue.inner.lock().unwrap_or_else(|p| p.into_inner());
        let mut state = self.job.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut removed = None;
        let retired = match &*state {
            State::Pending(_) => {
                removed = Some(
                    inner
                        .pending
                        .remove(self.pending_slot)
                        .expect("pending ticket owns one stable FIFO slot"),
                );
                inner.pending_body_bytes -= self.job.body_bytes;
                let retired = std::mem::replace(&mut *state, State::Canceled);
                if inner.leader {
                    queue.collect.notify_one();
                    queue.collected.notify_one();
                } else if let Some(head) = inner.pending.front() {
                    head.ready.notify_one();
                    head.notified.notify_one();
                }
                Some(retired)
            }
            State::Complete(_) => Some(std::mem::replace(&mut *state, State::Consumed)),
            State::Claimed => {
                *state = State::Abandoned;
                None
            }
            State::Abandoned | State::Canceled | State::Consumed => None,
        };
        drop(state);
        drop(inner);
        let _credits = retired.as_ref().map(|_| RetiredCredits {
            queue,
            calls: 1,
            bytes: self.job.charge,
        });
        drop(retired);
        drop(removed);
    }
}

impl<T, R> Clone for Queue<T, R> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            collect: Arc::clone(&self.collect),
            collected: Arc::clone(&self.collected),
            limits: Arc::clone(&self.limits),
        }
    }
}
#[path = "managed_group_queue_async.rs"]
mod asynchronous;
pub(crate) use asynchronous::isolated as execute_isolated;

#[cfg(test)]
#[path = "managed_group_queue_tests.rs"]
mod tests;
