//! Owned rendezvous waiters use notifications, not request/OS workers. Only a
//! bounded number of cohort executors enter the shared blocking runtime.
use super::*;
use std::{future::Future, pin::pin, sync::OnceLock};
use tokio::sync::Semaphore;

fn executors() -> &'static Arc<Semaphore> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(Semaphore::new(8)))
}

/// Queue input+reply reserve survives until the asynchronous socket writer
/// releases this value, including failed/slow delivery. No uncharged reply tail.
pub(crate) struct Completed<T, R> {
    pub(crate) reply: R,
    _charge: Retained<T, R>,
}
impl<T: Send + 'static, R: Send + 'static> Completed<T, R> {
    pub(crate) fn into_parts(self) -> (R, impl Send + 'static) {
        (self.reply, self._charge)
    }
}
struct Retained<T, R> {
    queue: Queue<T, R>,
    charge: usize,
}
impl<T, R> Drop for Retained<T, R> {
    fn drop(&mut self) {
        let mut inner = self.queue.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner.retained_calls -= 1;
        inner.retained_bytes -= self.charge;
    }
}

/// Before claim, cancellation releases leadership and leaves untouched jobs for
/// the next oldest waiter. Once claimed, the blocking executor owns completion.
struct Leadership<T, R> {
    queue: Queue<T, R>,
    armed: bool,
}
impl<T, R> Drop for Leadership<T, R> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut inner = self.queue.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner.leader = false;
        if let Some(head) = inner.pending.front() {
            head.ready.notify_one();
            head.notified.notify_one();
        }
    }
}

enum Step<R> {
    Complete(R),
    Lead,
    Wait,
}
impl<T: Send + 'static, R: Send + 'static> Ticket<T, R> {
    pub(crate) fn execute_async(
        mut self,
        run: impl FnOnce(&[T]) -> Vec<R> + Send + 'static,
        failed: impl Fn(&T) -> R + Send + 'static,
    ) -> impl Future<Output = Completed<T, R>> + Send {
        async move {
            let mut run = Some(run);
            let mut failed = Some(failed);
            loop {
                // enable() closes the state-inspection/await race; no spin or
                // per-request blocking task waits for another writer's save.
                let job = Arc::clone(&self.job);
                let mut ready = pin!(job.notified.notified());
                ready.as_mut().enable();
                match self.step() {
                    Step::Complete(reply) => {
                        self.consumed = true;
                        return Completed {
                            reply,
                            _charge: Retained {
                                queue: self.queue.clone(),
                                charge: self.job.charge,
                            },
                        };
                    }
                    Step::Wait => ready.await,
                    Step::Lead => {
                        let mut leadership = Leadership {
                            queue: self.queue.clone(),
                            armed: true,
                        };
                        let deadline = tokio::time::Instant::now() + self.queue.limits.wait;
                        loop {
                            let mut collected = pin!(self.queue.collected.notified());
                            collected.as_mut().enable();
                            let full = {
                                let inner =
                                    self.queue.inner.lock().unwrap_or_else(|p| p.into_inner());
                                inner.pending.len() >= self.queue.limits.group_calls
                                    || inner.pending_body_bytes >= self.queue.limits.group_bytes
                            };
                            if full {
                                break;
                            }
                            if tokio::time::timeout_at(deadline, collected).await.is_err() {
                                break;
                            }
                        }
                        let permit = Arc::clone(executors())
                            .acquire_owned()
                            .await
                            .expect("cohort executor semaphore never closes");
                        let (jobs, inputs) = self.claim();
                        let queue = self.queue.clone();
                        let run = run.take().expect("one leadership per request");
                        let failed = failed.take().expect("one panic handler per request");
                        // Completion belongs to this task even when its network
                        // waiter disappears after claim. Accepted work is not replayed.
                        let task = tokio::task::spawn_blocking(move || {
                            let _permit = permit;
                            let outcome =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    let replies = run(&inputs);
                                    assert_eq!(
                                        replies.len(),
                                        inputs.len(),
                                        "one reply per claimed request"
                                    );
                                    replies
                                }));
                            let replies =
                                outcome.unwrap_or_else(|_| inputs.iter().map(&failed).collect());
                            drop(inputs);
                            queue.complete(jobs, replies);
                        });
                        leadership.armed = false;
                        // The executor catches operation panics and fences via
                        // `failed`. Runtime shutdown is not a rejection/retry signal.
                        let _ = task.await;
                    }
                }
            }
        }
    }
    fn step(&self) -> Step<R> {
        let mut inner = self.queue.inner.lock().unwrap_or_else(|p| p.into_inner());
        let mut state = self.job.state.lock().unwrap_or_else(|p| p.into_inner());
        if let State::Complete(reply) = &mut *state {
            let reply = reply.take().expect("one async reply consumer");
            *state = State::Consumed;
            return Step::Complete(reply);
        }
        let first = inner
            .pending
            .front()
            .is_some_and(|head| Arc::ptr_eq(head, &self.job));
        if !inner.leader && first {
            inner.leader = true;
            Step::Lead
        } else {
            Step::Wait
        }
    }
    fn claim(&self) -> (Vec<Arc<Job<T, R>>>, Vec<T>) {
        let mut inner = self.queue.inner.lock().unwrap_or_else(|p| p.into_inner());
        let mut jobs = Vec::with_capacity(self.queue.limits.group_calls);
        let mut inputs = Vec::with_capacity(self.queue.limits.group_calls);
        let mut bytes = 0;
        while jobs.len() < self.queue.limits.group_calls {
            let Some(next) = inner.pending.front() else {
                break;
            };
            if next.body_bytes > self.queue.limits.group_bytes.saturating_sub(bytes) {
                break;
            }
            let next = inner.pending.pop_front().unwrap();
            let mut state = next.state.lock().unwrap_or_else(|p| p.into_inner());
            let State::Pending(input) = &mut *state else {
                unreachable!("pending FIFO");
            };
            inputs.push(input.take().expect("one input owner"));
            *state = State::Claimed;
            bytes += next.body_bytes;
            inner.pending_body_bytes -= next.body_bytes;
            drop(state);
            jobs.push(next);
        }
        (jobs, inputs)
    }
}
impl<T, R> Queue<T, R> {
    fn complete(&self, jobs: Vec<Arc<Job<T, R>>>, replies: Vec<R>) {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
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
        self.retire_abandoned(abandoned);
    }
}

pub(crate) async fn isolated<R: Send + 'static>(
    run: impl FnOnce() -> R + Send + 'static,
) -> Result<R, tokio::task::JoinError> {
    let permit = Arc::clone(executors())
        .acquire_owned()
        .await
        .expect("cohort executor semaphore never closes");
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        run()
    })
    .await
}
