use super::*;
use crate::read_tools::layout_children_values;

fn record(id: &str, parent: Option<&str>, name: Option<&str>, x: f64) -> LayoutRecord {
    LayoutRecord {
        id: id.into(),
        name: name.map(str::to_owned),
        type_label: "rectangle".into(),
        ancestors: parent.into_iter().map(str::to_owned).collect(),
        depth: u32::from(parent.is_some()),
        x,
        y: -0.0,
        w: 5.25,
        h: 2.0,
    }
}

#[test]
fn streaming_layout_is_byte_identical_to_value_tree_with_duplicate_parent_ids_and_offsets() {
    let mut records = vec![
        record("root", None, Some("quoted\"\\\n"), 100.25),
        record("peer", None, None, 2.0),
        record("child", Some("root"), None, 103.5),
        record("root", None, None, 0.0),
        record("last", Some("root"), Some(""), f64::NAN),
    ];
    records[1].w = f64::INFINITY;
    records[2].h = 9_007_199_254_740_992.0;
    let selected: Vec<_> = records.iter().collect();
    for (parent, offset) in [
        ("", (0.0, 0.0)),
        ("root", (100.25, -2.5)),
        ("absent", (0.0, 0.0)),
    ] {
        let expected =
            serde_json::json!({"layout":layout_children_values(&selected, parent, offset)})
                .to_string();
        assert_eq!(serialize_layout(&selected, parent, offset), expected);
    }
}
