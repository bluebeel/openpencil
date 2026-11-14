use super::*;
use crate::kiwi::FigValue;
use std::collections::HashSet;

fn object(fields: Vec<(&str, FigValue)>) -> FigValue {
    FigValue::Object(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn guid(id: u32) -> FigValue {
    FigValue::Guid(1, id)
}
fn size(w: f32, h: f32) -> FigValue {
    object(vec![("x", FigValue::Float(w)), ("y", FigValue::Float(h))])
}
fn node(kind: &str, id: u32, children: Vec<TreeNode>) -> TreeNode {
    TreeNode {
        figma: object(vec![
            ("type", FigValue::Str(kind.into())),
            ("name", FigValue::Str(format!("{kind}{id}").into())),
            ("guid", guid(id)),
            ("size", size(100.0, 100.0)),
        ]),
        children,
    }
}
fn instance(id: u32, master: u32) -> TreeNode {
    let mut n = node("INSTANCE", id, vec![]);
    n.figma
        .set("symbolData", object(vec![("symbolID", guid(master))]));
    n
}
fn assert_equivalent(tree: TreeNode, page: usize) {
    let borrowed = super::super::convert_tree_page(
        &tree,
        vec![],
        "all metadata",
        page,
        FigLayoutMode::Preserve,
    );
    let mut phases = vec![];
    let owned = convert_tree_page_owned(
        tree,
        vec![],
        "all metadata",
        page,
        FigLayoutMode::Preserve,
        &mut |p| phases.push(p),
    );
    assert_eq!(
        serde_json::to_value(&owned.document).unwrap(),
        serde_json::to_value(&borrowed.document).unwrap()
    );
    assert_eq!(owned.warnings, borrowed.warnings);
    assert_eq!(owned.image_blobs, borrowed.image_blobs);
    assert_eq!(phases, vec![FigImportPhase::ReleaseSource]);
}

#[test]
fn owned_page_keeps_cross_page_masters_nested_swaps_unknown_guid_containers_and_cycles() {
    let mut nested = instance(11, 30);
    nested.figma.set("overriddenSymbolID", guid(40));
    nested.figma.set(
        "futurePayload",
        object(vec![(
            "futureList",
            FigValue::Array(vec![object(vec![("futureTarget", guid(50))])].into()),
        )]),
    );
    let master_a = node("SYMBOL", 20, vec![nested]);
    let mut master_b = node("SYMBOL", 30, vec![node("RECTANGLE", 31, vec![])]);
    // The dependency walk closes cycles even inside currently unknown fields.
    master_b.figma.set("futureLink", guid(20));
    let mut master_c = node("SYMBOL", 40, vec![node("RECTANGLE", 41, vec![])]);
    master_c.figma.set(
        "futureAuthored",
        object(vec![
            ("bytes", FigValue::Bytes(vec![0, 17, 255].into())),
            ("unknown", FigValue::Null),
        ]),
    );
    let master_d = node("SYMBOL", 50, vec![node("RECTANGLE", 51, vec![])]);
    let unused = node("SYMBOL", 60, vec![node("RECTANGLE", 61, vec![])]);
    let selected = node("CANVAS", 2, vec![instance(10, 20)]);
    let tree = node(
        "DOCUMENT",
        1,
        vec![
            selected,
            node(
                "CANVAS",
                3,
                vec![master_a, master_b, master_c, master_d, unused],
            ),
        ],
    );
    let mut symbols = HashMap::new();
    collect_symbol_tree(&tree, &mut symbols);
    let wanted = super::super::dependencies::closure(&tree.children[0], &symbols);
    assert_eq!(
        wanted,
        [20, 30, 40, 50]
            .into_iter()
            .map(|id| format!("1:{id}"))
            .collect()
    );
    let authored = symbols["1:40"].clone();
    let mut pool = vec![];
    for child in tree.children[1].clone().children {
        masters(child, &mut pool, &wanted);
    }
    assert_eq!(
        pool.iter()
            .find(|n| n.figma.get("guid") == Some(&guid(40)))
            .unwrap()
            .figma,
        authored.figma
    );
    assert_equivalent(tree, 0);
}

#[test]
fn owned_page_seeds_wanted_masters_from_instances_on_other_pages() {
    let mut left = node("RECTANGLE", 21, vec![]);
    left.figma.set("size", size(20.0, 20.0));
    let mut right = node("RECTANGLE", 22, vec![]);
    right.figma.set("size", size(8.0, 8.0));
    right.figma.set(
        "transform",
        object(vec![
            ("m02", FigValue::Float(11.0)),
            ("m12", FigValue::Float(0.0)),
        ]),
    );
    let master = node("SYMBOL", 20, vec![left, right]);
    let mut rich = instance(70, 20);
    rich.figma.set(
        "derivedSymbolData",
        FigValue::Array(
            vec![object(vec![
                (
                    "guidPath",
                    object(vec![("guids", FigValue::Array(vec![guid(900)].into()))]),
                ),
                ("size", size(8.0, 8.0)),
                (
                    "transform",
                    object(vec![
                        ("m02", FigValue::Float(11.0)),
                        ("m12", FigValue::Float(0.0)),
                    ]),
                ),
            ])]
            .into(),
        ),
    );
    let tree = node(
        "DOCUMENT",
        1,
        vec![
            node("CANVAS", 2, vec![instance(10, 20)]),
            node("CANVAS", 3, vec![master, rich]),
        ],
    );
    let mut symbols = HashMap::new();
    collect_symbol_tree(&tree, &mut symbols);
    let wanted: HashSet<_> = ["1:20".to_string()].into();
    let mut filtered = HashMap::new();
    crate::instance::seed_assignments_for_symbols(&tree, &symbols, &mut filtered, &wanted);
    assert_eq!(filtered.get("1:20|1:900").map(String::as_str), Some("1:22"));
    let mut all = HashMap::new();
    crate::instance::seed_assignments_from_instances(&tree, &symbols, &mut all);
    assert_eq!(filtered, all);
    assert_equivalent(tree, 0);
}

#[test]
fn owned_page_uses_conservative_full_tree_fallback_for_duplicate_masters() {
    // Ordinary parsed trees deduplicate GUIDs; preserve legacy last-wins for manual trees too.
    let mut first = node("SYMBOL", 20, vec![node("RECTANGLE", 21, vec![])]);
    first.figma.set("future", FigValue::Str("first".into()));
    let second = node("SYMBOL", 20, vec![node("RECTANGLE", 22, vec![])]);
    let tree = node(
        "DOCUMENT",
        1,
        vec![
            node("CANVAS", 2, vec![instance(10, 20)]),
            node("CANVAS", 3, vec![first, second]),
        ],
    );
    assert_equivalent(tree, 0);
}

#[test]
fn owned_page_preserves_empty_and_legacy_out_of_range_results() {
    assert_equivalent(node("DOCUMENT", 1, vec![]), 0);
    assert_equivalent(
        node(
            "DOCUMENT",
            1,
            vec![node("CANVAS", 2, vec![node("RECTANGLE", 3, vec![])])],
        ),
        99,
    );
}
