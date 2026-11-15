use jian_core::document::{BorrowedNodeTree, NodeTree};
use jian_core::layout::LayoutEngine;
use jian_ops_schema::node::PenNode;
use serde_json::json;

#[test]
fn borrowed_layout_matches_owned_schemas_topology_and_rects_for_every_node_kind() {
    let kinds = [
        "frame",
        "group",
        "rectangle",
        "ellipse",
        "line",
        "polygon",
        "path",
        "text",
        "text_input",
        "image",
        "icon_font",
        "text_area",
        "select",
        "switch",
        "checkbox",
        "slider",
        "radio_group",
        "number_input",
        "progress",
        "tabs",
        "ref",
    ];
    let children: Vec<PenNode> = kinds
        .into_iter()
        .map(|kind| {
            serde_json::from_value(json!({
        "type":kind,"id":kind,"name":"all metadata","x":1.25,"y":-4.5,"width":32,"height":18,
        "rotation":35,"opacity":0.5,"locked":true,"theme":{"Mode":"Dark"},
        "ref":"component","content":"typed text","src":"data:image/png;base64,AA==",
        "d":"M0 0L1 1","polygonCount":5,"iconFontName":"heart","tabs":[],"options":[],
        "children":[{"type":"text","id":format!("{kind}-child"),"content":"nested"}]
    })).unwrap_or_else(|error| panic!("{kind}: {error}"))
        })
        .collect();
    let mut roots: Vec<PenNode> = vec![serde_json::from_value(json!({"type":"frame","id":"root",
        "x":20,"y":30,"layout":"horizontal","gap":7,"width":900,"height":200,
        "children":children}))
    .unwrap()];
    // Both a duplicate leaf ID within a root and a duplicate across roots retain
    // owned NodeTree's last-ID lookup while explicit child order stays intact.
    let extra: PenNode =
        serde_json::from_value(json!({"type":"rectangle","id":"rectangle","width":40,"height":20}))
            .unwrap();
    if let PenNode::Frame(root) = &mut roots[0] {
        root.children.as_mut().unwrap().push(extra.clone());
    }
    roots.push(extra);
    let mut owned = NodeTree::new();
    for root in &roots {
        owned.insert_subtree(root.clone(), None);
    }
    let borrowed = BorrowedNodeTree::from_roots(&roots);
    assert_eq!(borrowed.len(), owned.len());
    assert_eq!(borrowed.capacity(), borrowed.len());
    for (key, data) in &owned.nodes {
        assert_eq!(borrowed.schema(key), Some(&data.schema));
    }
    for (id, key) in &owned.by_id {
        assert_eq!(borrowed.by_id[id.as_str()], *key);
    }
    let mut a = LayoutEngine::new();
    let mut b = LayoutEngine::new();
    let ar = a.build(&owned).unwrap();
    let br = b.build_borrowed(&borrowed).unwrap();
    for (a_root, b_root) in ar.into_iter().zip(br) {
        a.compute(a_root, (900.0, 200.0)).unwrap();
        b.compute(b_root, (900.0, 200.0)).unwrap();
    }
    for key in owned.nodes.keys() {
        assert_eq!(a.node_rect(key), b.node_rect(key), "{key:?}");
    }
    println!(
        "borrowed nodes={} reserved={} owned schema bytes={}",
        borrowed.len(),
        borrowed.capacity(),
        std::mem::size_of::<PenNode>()
    );
}
