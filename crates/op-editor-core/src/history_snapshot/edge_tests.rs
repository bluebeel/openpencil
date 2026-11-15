use super::*;
use crate::test_support::{frame, rect, state_with};

#[test]
fn checkpoint_clones_share_flat_root_page_container_and_component_edges() {
    let flat: Vec<_> = (0..10_000)
        .map(|i| rect(&format!("leaf{i}"), "Leaf", 0., 0., 8., 8.))
        .collect();
    let mut state = state_with(flat);
    state.doc.pages = Some(vec![PenPage {
        id: "page".into(),
        name: "Page".into(),
        children: vec![frame(
            "frame",
            "Frame",
            0.,
            0.,
            8.,
            8.,
            vec![rect("child", "Child", 0., 0., 8., 8.)],
        )],
        background_color: None,
        state: None,
        lifecycle: None,
    }]);
    state.components.insert(Component {
        id: NodeId::new("component"),
        name: "Component".into(),
        root: state.doc.pages.as_ref().unwrap()[0].children[0].clone(),
    });
    let snapshot = state.snapshot_for_history();
    let clones: Vec<_> = (0..100).map(|_| snapshot.clone()).collect();
    let doc = &snapshot.doc;
    assert_eq!(Arc::strong_count(&doc.root_children), 101);
    assert_eq!(
        Arc::strong_count(&doc.root_children[0]),
        1,
        "cloning checkpoints never increments each leaf Arc"
    );
    for clone in &clones {
        assert!(Arc::ptr_eq(&doc.root_children, &clone.doc.root_children));
        assert!(Arc::ptr_eq(&doc.page_children, &clone.doc.page_children));
        assert_eq!(
            snapshot.components.edge_storage(),
            clone.components.edge_storage()
        );
        assert_eq!(
            doc.page_children[0][0].child_edge_storage(),
            clone.doc.page_children[0][0].child_edge_storage()
        );
    }
    let captured = SharedDoc::capture(&state.doc, Some(doc));
    assert!(Arc::ptr_eq(&doc.root_children, &captured.root_children));
    assert!(Arc::ptr_eq(&doc.page_children, &captured.page_children));
    let components = SharedComponents::capture(&state.components, Some(&snapshot.components));
    assert_eq!(
        components.edge_storage(),
        snapshot.components.edge_storage()
    );
    assert_eq!(
        serde_json::to_vec(doc).unwrap(),
        serde_json::to_vec(&state.doc).unwrap()
    );
    eprintln!(
        "edge-clone-proof {}",
        serde_json::json!({"rootNodes":10000,"checkpointClones":100,"sharedRootEdgeAllocationBytes":edges::storage(&doc.root_children).1,"legacyCopiedRootEdgeBytes":100*10000*std::mem::size_of::<Arc<SharedNode>>(),"additionalRootEdgeAllocations":0,"rootLeafArcStrongCount":Arc::strong_count(&doc.root_children[0])})
    );
}

#[test]
fn edge_array_cow_repair_detaches_only_touched_document_and_container_paths() {
    let mut state = state_with(vec![frame(
        "root",
        "Root",
        0.,
        0.,
        8.,
        8.,
        vec![rect("leaf", "Leaf", 0., 0., 8., 8.)],
    )]);
    state.doc.pages = Some(vec![PenPage {
        id: "page".into(),
        name: "Page".into(),
        children: vec![rect("page-leaf", "Other", 0., 0., 8., 8.)],
        background_color: None,
        state: None,
        lifecycle: None,
    }]);
    let original = SharedDoc::capture(&state.doc, None);
    let mut repaired = original.clone();
    let replacement: PenNode =
        serde_json::from_value(serde_json::json!({"type":"ref","id":"leaf","ref":"component"}))
            .unwrap();
    repaired.repair_swap(&NodeId::new("missing"), &replacement);
    assert!(Arc::ptr_eq(
        &original.root_children,
        &repaired.root_children
    ));
    repaired.repair_swap(&NodeId::new("leaf"), &replacement);
    assert!(!Arc::ptr_eq(
        &original.root_children,
        &repaired.root_children
    ));
    assert!(Arc::ptr_eq(
        &original.page_children,
        &repaired.page_children
    ));
    assert_ne!(
        original.root_children[0].child_edge_storage(),
        repaired.root_children[0].child_edge_storage()
    );
    assert!(matches!(
        original.root_children[0].children().unwrap()[0].metadata(),
        PenNode::Rectangle(_)
    ));
    assert!(matches!(
        repaired.root_children[0].children().unwrap()[0].metadata(),
        PenNode::Ref(_)
    ));
    assert_eq!(
        serde_json::to_vec(&original).unwrap(),
        serde_json::to_vec(&state.doc).unwrap()
    );
    assert_eq!(
        serde_json::to_vec(&repaired).unwrap(),
        serde_json::to_vec(&repaired.materialize()).unwrap()
    );
    let root_identity = edges::storage(&repaired.root_children).0;
    let replacement: PenNode = serde_json::from_value(
        serde_json::json!({"type":"ref","id":"page-leaf","ref":"component"}),
    )
    .unwrap();
    repaired.repair_swap(&NodeId::new("page-leaf"), &replacement);
    assert_eq!(root_identity, edges::storage(&repaired.root_children).0);
    assert!(!Arc::ptr_eq(
        &original.page_children,
        &repaired.page_children
    ));
    assert!(!Arc::ptr_eq(
        &original.page_children[0],
        &repaired.page_children[0]
    ));
    assert!(matches!(
        original.page_children[0][0].metadata(),
        PenNode::Rectangle(_)
    ));
    assert!(matches!(
        repaired.page_children[0][0].metadata(),
        PenNode::Ref(_)
    ));
}
