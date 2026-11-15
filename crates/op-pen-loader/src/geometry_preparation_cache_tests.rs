use super::*;
use crate::geometry_cache::values::{get_rectangles, rectangle_key};

#[test]
fn shared_rectangles_reuse_containing_root_but_not_geometry_or_changed_theme() {
    let store = Mutex::new(Store::new());
    let mut state = state();
    let geometry = read(&store, &state, 0, Some("a"), 0);
    let calls = AtomicUsize::new(0);
    let make = || {
        calls.fetch_add(1, Ordering::Relaxed);
        assert!(store.try_lock().is_ok());
        BTreeMap::from([("child".to_owned(), [0.0, 0.0, 5.0, 8.0])])
    };
    let parent = get_rectangles(&store, rectangle_key(&state, 0, "a"), make);
    let child = get_rectangles(&store, rectangle_key(&state, 0, "child"), make);
    assert!(Arc::ptr_eq(&parent, &child));
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(store.lock().unwrap().entries.len(), 2);
    assert!(!geometry.is_empty());
    state
        .ui
        .variables
        .active_theme
        .insert("Mode".into(), "Dark".into());
    let changed = get_rectangles(&store, rectangle_key(&state, 0, "a"), make);
    assert!(!Arc::ptr_eq(&parent, &changed));
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

#[test]
fn externally_held_evictions_remain_charged_and_block_new_shared_admission() {
    let store = Mutex::new(Store::new());
    let state = state();
    let calls = AtomicUsize::new(0);
    let make = || {
        calls.fetch_add(1, Ordering::Relaxed);
        let mut nodes = editor_state_to_page_item_geometry(&state, 0, Some("a"));
        nodes[0].id.reserve(MAX_BYTES * 3 / 5);
        nodes
    };
    let first = get_or_compute(&store, Key::new(&state, 0, Some("a"), 0), make);
    let second = get_or_compute(&store, Key::new(&state, 0, Some("a"), 1), make);
    {
        let locked = store.lock().unwrap();
        assert!(locked.entries.is_empty());
        assert_eq!(locked.retired.len(), 1);
        assert!(locked.bytes >= geometry_bytes(&first));
        assert!(locked.bytes + locked.base_bytes() <= MAX_BYTES);
    }
    let third = get_or_compute(&store, Key::new(&state, 0, Some("a"), 1), make);
    assert!(
        !Arc::ptr_eq(&second, &third),
        "uncached fallback must not hide a shared allocation"
    );
    drop((first, second, third));
    let _ = get_or_compute(&store, Key::new(&state, 0, Some("a"), 1), make);
    let locked = store.lock().unwrap();
    assert!(locked.retired.is_empty());
    assert_eq!(locked.entries.len(), 1);
    assert_eq!(calls.load(Ordering::Relaxed), 4);
}

#[test]
fn trusted_rectangle_reuse_keeps_exact_fresh_parent_geometry() {
    let state = state();
    for parent in ["a", "child", "b"] {
        let fresh = crate::editor_state_to_page_parent_geometry(&state, 0, parent);
        let reused = crate::computed_geometry::editor_state_to_page_parent_geometry_with_rect_reuse(
            &state, 0, parent,
        );
        fn assert_same(a: &[ComputedGeometry], b: &[ComputedGeometry]) {
            assert_eq!(a.len(), b.len());
            for (a, b) in a.iter().zip(b) {
                assert_eq!(a.id, b.id);
                assert_eq!(format!("{:?}", a.kind), format!("{:?}", b.kind));
                let bits = |r: op_editor_core::render_backend::Rect| {
                    [
                        r.origin.x.to_bits(),
                        r.origin.y.to_bits(),
                        r.size.x.to_bits(),
                        r.size.y.to_bits(),
                    ]
                };
                assert_eq!(bits(a.aggregate_bounds()), bits(b.aggregate_bounds()));
                assert_eq!(bits(a.visual_bounds()), bits(b.visual_bounds()));
                assert_same(&a.children, &b.children);
            }
        }
        assert_same(&fresh, &reused);
    }
}
