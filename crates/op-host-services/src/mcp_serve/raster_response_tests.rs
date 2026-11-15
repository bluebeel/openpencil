use super::*;
use crate::export::SharedRasterWorker;

#[test]
fn response_holds_capacity_after_tool_registry_drops() {
    let worker = SharedRasterWorker::new().unwrap();
    let response = RasterResponseScope::new();
    for _ in 0..2 {
        let tool = Arc::new(worker.try_acquire().unwrap());
        retain(&tool);
        drop(tool);
    }
    assert_eq!(worker.stats().admitted, 2);
    assert!(worker.try_acquire().is_err());
    drop(response);
    assert_eq!(worker.stats().admitted, 0);
    assert!(worker.try_acquire().is_ok());
}

#[test]
fn nested_response_scopes_release_only_their_own_resources() {
    let worker = SharedRasterWorker::new().unwrap();
    let outer = RasterResponseScope::new();
    retain(&Arc::new(worker.try_acquire().unwrap()));
    {
        let _inner = RasterResponseScope::new();
        retain(&Arc::new(worker.try_acquire().unwrap()));
        assert_eq!(worker.stats().admitted, 2);
    }
    assert_eq!(worker.stats().admitted, 1);
    drop(outer);
    assert_eq!(worker.stats().admitted, 0);
}

#[test]
fn unwind_does_not_leak_response_capacity() {
    let worker = SharedRasterWorker::new().unwrap();
    assert!(std::panic::catch_unwind(|| {
        let _response = RasterResponseScope::new();
        retain(&Arc::new(worker.try_acquire().unwrap()));
        panic!("connection failed");
    })
    .is_err());
    assert_eq!(worker.stats().admitted, 0);
    assert!(worker.try_acquire().is_ok());
}

#[test]
fn legacy_call_without_response_scope_does_not_retain_capacity() {
    let worker = SharedRasterWorker::new().unwrap();
    retain(&Arc::new(worker.try_acquire().unwrap()));
    assert_eq!(worker.stats().admitted, 0);
}

#[test]
fn out_of_order_drop_does_not_release_another_response() {
    let worker = SharedRasterWorker::new().unwrap();
    let outer = RasterResponseScope::new();
    let inner = RasterResponseScope::new();
    retain(&Arc::new(worker.try_acquire().unwrap()));
    drop(outer);
    assert_eq!(worker.stats().admitted, 1);
    drop(inner);
    assert_eq!(worker.stats().admitted, 0);
}
