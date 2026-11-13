use super::*;
use crate::test_support::{filled_rect, scene_with};
use op_editor_ui::Color;

fn scene() -> Arc<LayoutScene> {
    Arc::new(scene_with(vec![
        filled_rect("red", 8.0, 12.0, 24.0, 16.0, Color::RED),
        filled_rect("blue", 40.0, 12.0, 12.0, 20.0, Color::BLUE),
    ]))
}

fn node(id: &str) -> SharedRasterTarget {
    SharedRasterTarget::Node {
        page_id: "p1".into(),
        node_id: id.into(),
    }
}

#[test]
fn shared_worker_matches_synchronous_page_and_node_pixels_and_reuses_one_backend() {
    let worker = SharedRasterWorker::new().unwrap();
    let scene = scene();
    let page = scene.active_page().unwrap();
    let expected_page = crate::render_page_raster_bytes(page, RasterFormat::Png, 1.0).unwrap();
    let expected_node =
        crate::render_node_on_page_raster_bytes(page, "blue", RasterFormat::Png, 2.0).unwrap();
    let admission = worker.try_acquire().unwrap();
    for _ in 0..8 {
        assert_eq!(
            admission
                .render(
                    Arc::clone(&scene),
                    SharedRasterTarget::Page {
                        page_id: "p1".into()
                    },
                    RasterFormat::Png,
                    1.0
                )
                .unwrap(),
            expected_page
        );
        assert_eq!(
            admission
                .render(Arc::clone(&scene), node("blue"), RasterFormat::Png, 2.0)
                .unwrap(),
            expected_node
        );
    }
    assert_eq!(
        worker.stats(),
        RasterWorkerStats {
            admitted: 1,
            completed_jobs: 16,
            backend_initializations: 1
        }
    );
    drop(admission);
    assert_eq!(worker.stats().admitted, 0);
}

#[test]
fn concurrent_requests_have_two_admissions_and_honest_retry_backpressure() {
    let worker = SharedRasterWorker::new().unwrap();
    let scene = scene();
    let expected = crate::render_node_on_page_raster_bytes(
        scene.active_page().unwrap(),
        "red",
        RasterFormat::Png,
        1.0,
    )
    .unwrap();
    let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let (attempted, attempts) = mpsc::channel();
    let mut handles = Vec::new();
    for _ in 0..16 {
        let (worker, scene, release, attempted) = (
            worker.clone(),
            Arc::clone(&scene),
            Arc::clone(&release),
            attempted.clone(),
        );
        handles.push(std::thread::spawn(move || {
            let admission = worker.try_acquire();
            attempted.send(admission.is_ok()).unwrap();
            match admission {
                Ok(admission) => {
                    let (lock, changed) = &*release;
                    let _released = changed
                        .wait_while(lock.lock().unwrap(), |released| !*released)
                        .unwrap();
                    Some(
                        admission
                            .render(scene, node("red"), RasterFormat::Png, 1.0)
                            .unwrap(),
                    )
                }
                Err(error) => {
                    assert_eq!(error, ExportError::RenderWorkerBusy { max_admitted: 2 });
                    None
                }
            }
        }));
    }
    let accepted = (0..16).filter(|_| attempts.recv().unwrap()).count();
    assert_eq!(accepted, 2);
    assert_eq!(worker.stats().admitted, 2);
    // 1000 additional logical agents are rejected without creating scenes,
    // render threads, queued jobs, or encoded output buffers.
    for _ in 0..1000 {
        assert!(matches!(
            worker.try_acquire(),
            Err(ExportError::RenderWorkerBusy { .. })
        ));
    }
    let (lock, changed) = &*release;
    *lock.lock().unwrap() = true;
    changed.notify_all();
    for handle in handles {
        if let Some(bytes) = handle.join().unwrap() {
            assert_eq!(bytes, expected);
        }
    }
    assert_eq!(
        worker.stats(),
        RasterWorkerStats {
            admitted: 0,
            completed_jobs: 2,
            backend_initializations: 1
        }
    );
    assert!(
        worker.try_acquire().is_ok(),
        "dropping requests restores capacity"
    );
}

#[test]
fn worker_preserves_structured_identity_and_output_size_errors() {
    let worker = SharedRasterWorker::new().unwrap();
    let admission = worker.try_acquire().unwrap();
    let scene = scene();
    assert_eq!(
        admission
            .render(Arc::clone(&scene), node("absent"), RasterFormat::Png, 1.0)
            .unwrap_err(),
        ExportError::NodeNotFoundOnPage {
            node_id: "absent".into(),
            page_id: "p1".into()
        }
    );
    assert_eq!(
        admission
            .render(
                scene,
                SharedRasterTarget::Page {
                    page_id: "missing".into()
                },
                RasterFormat::Png,
                1.0
            )
            .unwrap_err(),
        ExportError::PageNotFound {
            page_id: "missing".into()
        }
    );
    let huge = Arc::new(scene_with(vec![filled_rect(
        "huge",
        0.0,
        0.0,
        20_000.0,
        2.0,
        Color::RED,
    )]));
    assert_eq!(
        admission
            .render(huge, node("huge"), RasterFormat::Png, 1.0)
            .unwrap_err(),
        ExportError::OutputTooLarge {
            width_px: 20_000,
            height_px: 2
        }
    );
}
