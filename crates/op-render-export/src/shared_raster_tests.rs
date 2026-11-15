use super::*;
use crate::test_support::{filled_rect, scene_with};
use op_editor_ui::Color;

#[test]
fn shared_worker_streams_partial_file_batch_and_reuses_renderer_for_bytes() {
    let worker = SharedRasterWorker::new().unwrap();
    let scene = scene();
    let admission = worker.try_acquire().unwrap();
    let directory = std::env::temp_dir().join(format!("op-worker-files-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let targets = ["red", "absent", "blue"]
        .into_iter()
        .map(|id| crate::RasterFileTarget {
            node_id: id.into(),
            path: directory.join(format!("{id}.png")),
        })
        .collect();
    let results = admission
        .export_files(Arc::clone(&scene), targets, RasterFormat::Png, 1.0)
        .unwrap();
    assert_eq!(results.len(), 3);
    assert!(results[0].is_ok() && results[2].is_ok());
    assert_eq!(
        results[1],
        Err(ExportError::NodeNotFoundOnActivePage {
            node_id: "absent".into()
        })
    );
    assert!(!directory.join("absent.png").exists());
    let bytes = admission
        .render(Arc::clone(&scene), node("blue"), RasterFormat::Png, 1.0)
        .unwrap();
    assert_eq!(std::fs::read(directory.join("blue.png")).unwrap(), bytes);
    assert_eq!(worker.stats().backend_initializations, 1);
    assert_eq!(worker.stats().completed_jobs, 2);
    drop(admission);
    assert_eq!(worker.stats().admitted, 0);
    std::fs::remove_dir_all(directory).unwrap();
}

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
