use super::*;

#[test]
fn conversion_excludes_renders_and_waits_for_both_render_responses() {
    let gate = SharedWorkGate::new();
    let first = gate.try_render().unwrap();
    let second = gate.try_render().unwrap();
    assert!(gate.try_render().is_none());
    assert!(gate.try_conversion().is_none());
    drop(first);
    assert!(gate.try_conversion().is_none());
    drop(second);
    let conversion = gate.try_conversion().unwrap();
    assert!(gate.try_render().is_none());
    assert!(gate.try_conversion().is_none());
    drop(conversion);
    assert!(gate.try_render().is_some());
}

#[test]
fn worker_rejection_releases_its_request_counter() {
    let gate = SharedWorkGate::new();
    let worker = crate::shared_raster::SharedRasterWorker::with_work_gate(gate.clone()).unwrap();
    let conversion = gate.try_conversion().unwrap();
    assert!(matches!(
        worker.try_acquire(),
        Err(crate::ExportError::SharedWorkBusy)
    ));
    assert_eq!(worker.stats().admitted, 0);
    drop(conversion);
    let request = worker.try_acquire().unwrap();
    assert!(gate.try_conversion().is_none());
    drop(request);
    assert!(gate.try_conversion().is_some());
}

#[test]
fn shared_gate_never_overlaps_exclusive_work_under_contention() {
    let gate = SharedWorkGate::new();
    let renders = AtomicUsize::new(0);
    let conversions = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for thread in 0..32 {
            let (gate, renders, conversions) = (&gate, &renders, &conversions);
            scope.spawn(move || {
                for iteration in 0..2000 {
                    if (thread + iteration) % 5 == 0 {
                        if let Some(lease) = gate.try_conversion() {
                            assert_eq!(conversions.fetch_add(1, Ordering::SeqCst), 0);
                            assert_eq!(renders.load(Ordering::SeqCst), 0);
                            conversions.fetch_sub(1, Ordering::SeqCst);
                            drop(lease);
                        }
                    } else if let Some(lease) = gate.try_render() {
                        assert!(renders.fetch_add(1, Ordering::SeqCst) < MAX_RENDER_ADMISSIONS);
                        assert_eq!(conversions.load(Ordering::SeqCst), 0);
                        renders.fetch_sub(1, Ordering::SeqCst);
                        drop(lease);
                    }
                }
            });
        }
    });
    assert!(gate.try_conversion().is_some());
}

#[test]
fn unwind_releases_all_capacity() {
    let gate = SharedWorkGate::new();
    for conversion in [false, true] {
        assert!(std::panic::catch_unwind(|| {
            let _lease = if conversion {
                gate.try_conversion().unwrap()
            } else {
                gate.try_render().unwrap()
            };
            panic!("request failed");
        })
        .is_err());
        assert!(gate.try_conversion().is_some());
    }
}
