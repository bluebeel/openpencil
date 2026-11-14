use super::*;
use op_editor_core::{EditorCommand, NodeId};
use std::sync::atomic::{AtomicUsize, Ordering};

fn state() -> EditorState {
    EditorState::from_document(
        jian_ops_schema::load_str(
            r#"{
     "version":"1.0.0","pages":[
      {"id":"p","name":"P","children":[
       {"id":"a","type":"frame","x":10,"width":100,"height":80,"children":[
        {"id":"child","type":"rectangle","width":5,"height":8}]},
       {"id":"b","type":"rectangle","x":200,"width":20,"height":30}]},
      {"id":"other","name":"Other","children":[{"id":"c","type":"rectangle","width":7,"height":9}]}]
    }"#,
        )
        .unwrap()
        .value,
    )
}
fn read(
    store: &Mutex<Store>,
    state: &EditorState,
    page: usize,
    item: Option<&str>,
    font: u64,
) -> Arc<Vec<ComputedGeometry>> {
    get_or_compute(store, Key::new(state, page, item, font), || {
        assert!(
            store.try_lock().is_ok(),
            "cache writer lock must not cross computation"
        );
        editor_state_to_page_item_geometry(state, page, item)
    })
}
#[test]
fn trusted_geometry_reuse_invalidates_edits_undo_reload_clones_and_fresh_states() {
    let store = Mutex::new(Store::new());
    let mut s = state();
    let a = read(&store, &s, 0, Some("child"), 0);
    assert!(
        Arc::ptr_eq(&a, &read(&store, &s, 0, Some("a"), 0)),
        "same containing-root scope shares data"
    );
    s.commit_history();
    assert!(s.apply(EditorCommand::UpdateNode {
        node_id: NodeId::new("a"),
        x: Some(55),
        y: None,
        width: None,
        height: None,
        name: None,
        fill_hex: None,
        page_id: Some("p".into())
    }));
    let moved = read(&store, &s, 0, Some("child"), 0);
    assert!(!Arc::ptr_eq(&a, &moved));
    assert_ne!(a[0].aggregate_bounds(), moved[0].aggregate_bounds());
    assert!(s.apply(EditorCommand::Undo));
    let undo = read(&store, &s, 0, Some("child"), 0);
    assert!(!Arc::ptr_eq(&a, &undo));
    assert_eq!(a[0].aggregate_bounds(), undo[0].aggregate_bounds());
    let cloned = s.clone();
    assert!(!Weak::ptr_eq(
        &s.derived_cache_owner(),
        &cloned.derived_cache_owner()
    ));
    assert!(!Arc::ptr_eq(
        &undo,
        &read(&store, &cloned, 0, Some("child"), 0)
    ));
    let before = read(&store, &s, 0, Some("a"), 0);
    let doc = s.doc.clone();
    s.replace_document(doc);
    assert!(!Arc::ptr_eq(&before, &read(&store, &s, 0, Some("a"), 0)));
    assert!(!Arc::ptr_eq(&a, &read(&store, &state(), 0, Some("a"), 0)));
}
#[test]
fn trusted_cache_checks_page_scope_theme_view_and_font_inputs() {
    let store = Mutex::new(Store::new());
    let mut s = state();
    let a = read(&store, &s, 0, Some("a"), 0);
    assert!(!Arc::ptr_eq(&a, &read(&store, &s, 0, Some("b"), 0)));
    assert!(!Arc::ptr_eq(&a, &read(&store, &s, 1, None, 0)));
    assert!(
        !Arc::ptr_eq(&a, &read(&store, &s, 0, Some("a"), 1)),
        "font generation miss"
    );
    s.ui.variables
        .active_theme
        .insert("Mode".into(), "Dark".into());
    let theme = read(&store, &s, 0, Some("a"), 0);
    assert!(!Arc::ptr_eq(&a, &theme));
    s.editor_ui.preserve_authored_geometry = true;
    let authored = read(&store, &s, 0, Some("a"), 0);
    assert!(!Arc::ptr_eq(&theme, &authored));
    s.editor_ui.workspace.visible = true;
    let workspace = read(&store, &s, 0, Some("a"), 0);
    assert!(!Arc::ptr_eq(&authored, &workspace));
    s.editor_ui.preview.mode = true;
    let preview = read(&store, &s, 0, Some("a"), 0);
    assert!(!Arc::ptr_eq(&workspace, &preview));
    s.ui.active_page_index = 1;
    assert!(!Arc::ptr_eq(&preview, &read(&store, &s, 0, Some("a"), 0)));
}
#[test]
fn cache_uses_weak_owners_and_charges_reserved_capacity_before_admission() {
    let store = Mutex::new(Store::new());
    let s = state();
    let owner = s.derived_cache_owner();
    let held = read(&store, &s, 0, Some("a"), 0);
    drop(s);
    assert!(owner.upgrade().is_none());
    let fresh = state();
    let _ = read(&store, &fresh, 0, Some("a"), 0);
    assert!(store
        .lock()
        .unwrap()
        .entries
        .iter()
        .all(|entry| !Weak::ptr_eq(&entry.key.owner, &owner)));
    drop(held);
    let oversize = Mutex::new(Store::new());
    let calls = AtomicUsize::new(0);
    let make = || {
        calls.fetch_add(1, Ordering::Relaxed);
        let mut data = editor_state_to_page_item_geometry(&fresh, 0, Some("a"));
        data[0].id.reserve(MAX_BYTES);
        data
    };
    let one = get_or_compute(&oversize, Key::new(&fresh, 0, Some("a"), 0), make);
    assert!(geometry_bytes(&one) > MAX_BYTES);
    assert!(oversize.lock().unwrap().entries.is_empty());
    drop(one);
    let _ = get_or_compute(&oversize, Key::new(&fresh, 0, Some("a"), 0), make);
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}
#[test]
fn shared_lru_has_global_entry_and_byte_bounds() {
    let store = Mutex::new(Store::new());
    let s = state();
    let oldest = read(&store, &s, 0, Some("a"), 0);
    for font in 1..=MAX_ENTRIES as u64 {
        let _ = read(&store, &s, 0, Some("a"), font);
    }
    {
        let locked = store.lock().unwrap();
        assert_eq!(locked.entries.len(), MAX_ENTRIES);
        assert!(locked.bytes + locked.base_bytes() <= MAX_BYTES);
    }
    assert!(
        !Arc::ptr_eq(&oldest, &read(&store, &s, 0, Some("a"), 0)),
        "oldest evicted"
    );
}
