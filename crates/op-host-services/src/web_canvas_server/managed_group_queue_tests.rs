use super::*;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Barrier,
};

fn queue(
    calls: usize,
    bytes: usize,
    group_calls: usize,
    group_bytes: usize,
) -> Queue<usize, usize> {
    Queue::new(Limits {
        calls,
        bytes,
        group_calls,
        group_bytes,
        wait: Duration::from_millis(2),
    })
}

#[test]
fn refused_inputs_remain_unexecuted_and_canceled_tickets_release_charge() {
    let queue = queue(2, 20, 2, 10);
    assert!(queue.submit(10, 11, 11).is_err());
    let first = queue.submit(1, 5, 10).ok().unwrap();
    let second = queue.submit(2, 5, 10).ok().unwrap();
    assert_eq!(queue.submit(3, 1, 1).err(), Some(3));
    assert_eq!(queue.retained(), (2, 20));
    drop(second);
    assert_eq!(queue.retained(), (1, 10));
    drop(first);
    assert_eq!(queue.retained(), (0, 0));
}

#[test]
fn fifo_group_runs_once_and_no_reply_precedes_commit() {
    let queue = queue(32, 320, 8, 80);
    let barrier = Barrier::new(8);
    let committed = AtomicBool::new(false);
    let saves = AtomicUsize::new(0);
    let tickets: Vec<_> = (0..8)
        .map(|n| queue.submit(n, 10, 10).ok().unwrap())
        .collect();
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for ticket in tickets {
            handles.push(scope.spawn(|| {
                barrier.wait();
                let reply = ticket.execute(
                    |inputs| {
                        assert_eq!(inputs, &[0, 1, 2, 3, 4, 5, 6, 7]);
                        assert!(!committed.load(Ordering::Acquire));
                        saves.fetch_add(1, Ordering::AcqRel);
                        committed.store(true, Ordering::Release);
                        inputs.iter().map(|n| n + 100).collect()
                    },
                    |_| usize::MAX,
                );
                assert!(committed.load(Ordering::Acquire));
                reply
            }));
        }
        let replies: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(replies, (100..108).collect::<Vec<_>>());
    });
    assert_eq!(saves.load(Ordering::Acquire), 1);
    assert_eq!(queue.retained(), (0, 0));
}

#[test]
fn byte_bound_splits_groups_without_changing_fifo_order() {
    let queue = queue(10, 100, 10, 10);
    let tickets: Vec<_> = (0..6)
        .map(|n| queue.submit(n, 5, 10).ok().unwrap())
        .collect();
    let order = Mutex::new(Vec::new());
    let groups = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let handles: Vec<_> = tickets
            .into_iter()
            .map(|ticket| {
                scope.spawn(|| {
                    ticket.execute(
                        |inputs| {
                            assert_eq!(inputs.len(), 2);
                            order.lock().unwrap().extend_from_slice(inputs);
                            groups.fetch_add(1, Ordering::AcqRel);
                            inputs.to_vec()
                        },
                        |_| usize::MAX,
                    )
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
    });
    assert_eq!(*order.lock().unwrap(), vec![0, 1, 2, 3, 4, 5]);
    assert_eq!(groups.load(Ordering::Acquire), 3);
    assert_eq!(queue.retained(), (0, 0));
}

#[test]
fn executor_panic_answers_every_claimed_input_without_reexecution() {
    let queue = queue(4, 40, 4, 40);
    let attempts = AtomicUsize::new(0);
    let tickets: Vec<_> = (0..4)
        .map(|n| queue.submit(n, 10, 10).ok().unwrap())
        .collect();
    std::thread::scope(|scope| {
        let handles: Vec<_> = tickets
            .into_iter()
            .map(|ticket| {
                scope.spawn(|| {
                    ticket.execute(
                        |_| {
                            attempts.fetch_add(1, Ordering::AcqRel);
                            panic!("injected executor failure after its rollback guard");
                        },
                        |n| n + 200,
                    )
                })
            })
            .collect();
        let replies: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(replies, vec![200, 201, 202, 203]);
    });
    assert_eq!(attempts.load(Ordering::Acquire), 1);
    assert_eq!(queue.retained(), (0, 0));
}

#[test]
fn canceled_head_does_not_strand_remaining_waiter() {
    let queue = queue(2, 20, 2, 20);
    let first = queue.submit(1, 10, 10).ok().unwrap();
    let second = queue.submit(2, 10, 10).ok().unwrap();
    drop(first);
    assert_eq!(second.execute(|inputs| inputs.to_vec(), |_| usize::MAX), 2);
    assert_eq!(queue.retained(), (0, 0));
}

#[test]
fn a_waiter_dropped_after_claim_does_not_replay_or_cancel_committed_work() {
    let queue = queue(2, 20, 2, 20);
    let first = queue.submit(1, 10, 10).ok().unwrap();
    let second = queue.submit(2, 10, 10).ok().unwrap();
    let (begun_tx, begun_rx) = std::sync::mpsc::channel();
    let (continue_tx, continue_rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let handle = scope.spawn(move || {
            first.execute(
                |inputs| {
                    assert_eq!(inputs, &[1, 2]);
                    begun_tx.send(()).unwrap();
                    continue_rx.recv().unwrap();
                    inputs.to_vec()
                },
                |_| usize::MAX,
            )
        });
        begun_rx.recv().unwrap();
        drop(second);
        assert_eq!(
            queue.retained(),
            (2, 20),
            "claim owns charge through commit"
        );
        continue_tx.send(()).unwrap();
        assert_eq!(handle.join().unwrap(), 1);
    });
    assert_eq!(queue.retained(), (0, 0));
}

#[test]
fn execution_pins_are_released_before_a_completed_response_is_visible() {
    struct Input {
        id: usize,
        dropping: Option<std::sync::mpsc::Sender<()>>,
        release: Option<std::sync::mpsc::Receiver<()>>,
    }
    impl Drop for Input {
        fn drop(&mut self) {
            if let Some(started) = self.dropping.take() {
                started.send(()).unwrap();
                self.release.take().unwrap().recv().unwrap();
            }
        }
    }
    let queue: Queue<Input, usize> = Queue::new(Limits {
        calls: 2,
        bytes: 20,
        group_calls: 2,
        group_bytes: 20,
        wait: Duration::ZERO,
    });
    let (dropping_tx, dropping_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let first = queue
        .submit(
            Input {
                id: 1,
                dropping: Some(dropping_tx),
                release: Some(release_rx),
            },
            10,
            10,
        )
        .ok()
        .unwrap();
    let second = queue
        .submit(
            Input {
                id: 2,
                dropping: None,
                release: None,
            },
            10,
            10,
        )
        .ok()
        .unwrap();
    let watched = Arc::clone(&second.job);
    std::thread::scope(|scope| {
        let handle = scope.spawn(move || {
            first.execute(
                |inputs| inputs.iter().map(|input| input.id).collect(),
                |_| usize::MAX,
            )
        });
        dropping_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let still_claimed = matches!(*watched.state.lock().unwrap(), State::Claimed);
        release_tx.send(()).unwrap();
        assert_eq!(handle.join().unwrap(), 1);
        assert_eq!(
            second.execute(|_| panic!("already completed input"), |_| usize::MAX),
            2
        );
        assert!(
            still_claimed,
            "a sequential client must never see completion before its execution pin is released"
        );
    });
    assert_eq!(queue.retained(), (0, 0));
}
