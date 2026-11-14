use super::*;
use crate::common::FigLayoutMode;
use crate::node_mapper::{convert_tree_page, resolve_style_references_with_mode};

fn object(fields: Vec<(&str, FigValue)>) -> FigValue {
    FigValue::Object(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn node(id: u32, parent: Option<u32>, kind: &str) -> FigValue {
    let mut n = object(vec![
        ("guid", FigValue::Guid(1, id)),
        ("type", FigValue::Str(kind.into())),
        ("name", FigValue::Str(format!("{kind}-{id}").into())),
        (
            "size",
            object(vec![
                ("x", FigValue::Float(50.0)),
                ("y", FigValue::Float(20.0)),
            ]),
        ),
        ("futureAuthored", FigValue::Bytes(vec![0, 128, 255].into())),
    ]);
    if let Some(parent) = parent {
        n.set(
            "parentIndex",
            object(vec![
                ("guid", FigValue::Guid(1, parent)),
                ("position", FigValue::Str(format!("{id:04}").into())),
            ]),
        );
    }
    n
}
fn instance(id: u32, parent: u32, master: u32) -> FigValue {
    let mut n = node(id, Some(parent), "INSTANCE");
    n.set(
        "symbolData",
        object(vec![("symbolID", FigValue::Guid(1, master))]),
    );
    n
}
fn metadata(n: &FigValue) -> FigValue {
    let FigValue::Object(fields) = n else {
        unreachable!()
    };
    object(
        fields
            .iter()
            .filter_map(|(key, value)| match key.as_ref() {
                "guid" | "type" | "phase" | "parentIndex" | "name" | "styleType"
                | "componentPropRefs" => Some((key.as_ref(), value.clone())),
                "symbolData" => Some((
                    key.as_ref(),
                    object(vec![("symbolID", value.get("symbolID").unwrap().clone())]),
                )),
                _ => None,
            })
            .collect(),
    )
}
fn fixture() -> Vec<FigValue> {
    let mut nodes = vec![
        node(1, None, "DOCUMENT"),
        node(2, Some(1), "CANVAS"),
        node(3, Some(1), "CANVAS"),
        instance(10, 2, 20),
        node(20, Some(3), "SYMBOL"),
        instance(21, 20, 30),
        node(30, Some(3), "SYMBOL"),
        node(31, Some(30), "RECTANGLE"),
        node(40, Some(3), "SYMBOL"),
        node(41, Some(40), "RECTANGLE"),
        node(50, None, "STYLE"),
        node(60, Some(3), "TEXT"),
        instance(70, 3, 20),
        node(90, Some(3), "SYMBOL"),
        node(91, Some(90), "RECTANGLE"),
    ];
    nodes[5].set(
        "futureUnknown",
        object(vec![(
            "nested",
            FigValue::Array(vec![FigValue::Guid(1, 40)].into()),
        )]),
    );
    nodes[3].set("futureFontOwner", FigValue::Guid(1, 60));
    nodes[10].set("styleType", FigValue::Str("TEXT".into()));
    nodes[12].set("futureUnrelated", FigValue::Guid(1, 90));
    nodes
}
#[test]
fn selected_wire_closure_keeps_unknown_dependencies_global_evidence_and_ancestor_context() {
    let full = fixture();
    let meta: Vec<_> = full.iter().map(metadata).collect();
    let topology = Topology::new(&meta).unwrap();
    assert_eq!(topology.pages(&meta)[0].child_count, 1);
    let mut hydrated = HashSet::new();
    let selected = select(meta, &topology, 0, |i, n| {
        assert!(hydrated.insert(i));
        *n = full[i].clone();
    });
    for id in [1, 2, 3, 10, 20, 21, 30, 31, 40, 41, 50, 60, 70] {
        let i = full
            .iter()
            .position(|n| {
                guid(n)
                    == Some(FigGuid {
                        session_id: 1,
                        local_id: id,
                    })
            })
            .unwrap();
        assert!(hydrated.contains(&i), "required source {id}");
        assert_eq!(selected[i], full[i]);
    }
    for id in [90, 91] {
        let i = full
            .iter()
            .position(|n| {
                guid(n)
                    == Some(FigGuid {
                        session_id: 1,
                        local_id: id,
                    })
            })
            .unwrap();
        assert!(
            !hydrated.contains(&i),
            "auxiliary evidence is not a render dependency"
        );
    }
    for layout in [FigLayoutMode::Preserve, FigLayoutMode::OpenPencil] {
        let convert = |mut nodes: Vec<FigValue>| {
            resolve_style_references_with_mode(&mut nodes, layout);
            let tree = crate::tree::build_tree_owned(nodes).unwrap();
            convert_tree_page(&tree, vec![], "Selection", 0, layout)
        };
        let (a, b) = (convert(full.clone()), convert(selected.clone()));
        assert_eq!(
            serde_json::to_value(a.document).unwrap(),
            serde_json::to_value(b.document).unwrap()
        );
        assert_eq!(a.warnings, b.warnings);
    }
}
#[test]
fn ambiguous_or_cyclic_topology_uses_eager_fallback_and_invalid_page_hydrates_everything() {
    let full = fixture();
    let mut duplicate = full.clone();
    duplicate.push(full[4].clone());
    assert!(Topology::new(&duplicate).is_none());
    let mut cycle = full.clone();
    cycle[0].set("parentIndex", object(vec![("guid", FigValue::Guid(1, 2))]));
    assert!(Topology::new(&cycle).is_none());
    let meta: Vec<_> = full.iter().map(metadata).collect();
    let topology = Topology::new(&meta).unwrap();
    let result = select(meta, &topology, usize::MAX, |i, n| *n = full[i].clone());
    assert_eq!(result, full);
}
