use super::*;
use jian_ops_schema::{
    node::{NumberOrExpression, PenNode},
    sizing::SizingBehavior,
};
use std::sync::Arc;
fn read(text: &str) -> PenDocument {
    serde_json::from_str(text).unwrap()
}
fn check(a: &PenDocument, b: &PenDocument) -> bool {
    let first = SharedDoc::capture(a, None);
    // equals_node is the UNCHANGED independent PartialEq+sensitivity reference.
    let expected = first.root_children()[0].equals_node(&b.children[0]);
    let next = SharedDoc::capture(b, Some(&first));
    assert_eq!(
        Arc::ptr_eq(&first.root_children()[0], &next.root_children()[0]),
        expected
    );
    assert_eq!(
        serde_json::to_vec(&next).unwrap(),
        serde_json::to_vec(b).unwrap()
    );
    assert_eq!(
        serde_json::to_vec(&next.materialize()).unwrap(),
        serde_json::to_vec(b).unwrap()
    );
    expected
}
fn set(doc: &mut PenDocument, field: usize, v: f64) {
    let PenNode::Rectangle(r) = &mut doc.children[0] else {
        unreachable!()
    };
    match field {
        0 => r.base.x = Some(v),
        1 => r.base.y = Some(v),
        2 => r.base.rotation = Some(v),
        3 => r.base.opacity = Some(NumberOrExpression::Number(v)),
        4 => r.container.width = Some(SizingBehavior::Number(v)),
        5 => r.container.height = Some(SizingBehavior::Number(v)),
        6 => r.container.gap = Some(NumberOrExpression::Number(v)),
        7 => r.container.limits.min_width = Some(v),
        8 => r.container.limits.max_width = Some(v),
        9 => r.container.limits.min_height = Some(v),
        10 => r.container.limits.max_height = Some(v),
        11 => {
            if let Some(jian_ops_schema::style::PenFill::Solid(s)) =
                r.container.fill.as_mut().and_then(|v| v.first_mut())
            {
                s.opacity = Some(v)
            }
        }
        _ => unreachable!(),
    }
}
#[test]
fn typed_leaf_capture_matches_exact_reference_for_full_schema_and_bit_boundaries() {
    let fixture = r##"{"version":"1.0.0","children":[{"type":"rectangle","id":"r","name":"all geometry","width":8,"height":8,"fill":[{"type":"solid","color":"#123456"}]}]}"##;
    let base = read(fixture);
    let mut cases = 0;
    for field in 0..12 {
        for (a, b) in [
            (0.0, -0.0),
            (-0.0, 0.0),
            (1.25, 1.25),
            (f64::INFINITY, f64::INFINITY),
            (f64::INFINITY, f64::NEG_INFINITY),
            (f64::NAN, f64::NAN),
            (f64::from_bits(1), 0.0),
            (f64::from_bits(0x8000000000000001), -0.0),
        ] {
            let (mut x, mut y) = (base.clone(), base.clone());
            set(&mut x, field, a);
            set(&mut y, field, b);
            check(&x, &y);
            cases += 1;
        }
    }
    let mut seed = 0x8b56_3b29_u64;
    for _ in 0..256 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let (mut a, mut b) = (base.clone(), base.clone());
        set(&mut a, 0, f64::from_bits(seed));
        set(&mut b, 0, f64::from_bits(seed));
        check(&a, &b);
        cases += 1;
    }
    for kind in [
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
    ] {
        let source = serde_json::json!({"version":"1.0.0","children":[{"type":kind,"id":"n","name":"full metadata","x":1.25,"y":-4.5,"rotation":35,"visible":false,"opacity":0.5,"locked":true,"theme":{"Mode":"Dark"},"maskType":"alpha","blendMode":"multiply","ref":"component","content":"typed text","src":"https://example.com/image.png","d":"M0 0L1 1","polygonCount":5,"iconFontName":"heart","tabs":[],"options":[],"children":[{"type":"frame","id":"inner","children":[{"type":"text","id":"leaf","content":"nested"}]}]}]});
        let a: PenDocument =
            serde_json::from_value(source).unwrap_or_else(|e| panic!("{kind}:{e}"));
        assert!(check(&a, &a));
        let mut b = a.clone();
        b.children[0].base_mut().name = Some("changed complete metadata".into());
        assert!(!check(&a, &b));
        cases += 2;
    }
    let a = read(
        r#"{"version":"1.0.0","children":[{"type":"rectangle","id":"r","state":{"v":{"type":"object","default":{"z":1,"a":2}}}}]}"#,
    );
    let b = read(
        r#"{"version":"1.0.0","children":[{"type":"rectangle","id":"r","state":{"v":{"type":"object","default":{"a":2,"z":1}}}}]}"#,
    );
    assert_eq!(a, b);
    assert!(!check(&a, &b));
    cases += 1;
    for kind in ["rectangle", "frame", "group", "tabs", "ref"] {
        let a = read(&format!(
            r#"{{"version":"1.0.0","children":[{{"type":"{kind}","id":"n","ref":"master"}}]}}"#
        ));
        let b = read(&format!(
            r#"{{"version":"1.0.0","children":[{{"type":"{kind}","id":"n","ref":"master","children":[]}}]}}"#
        ));
        assert!(!check(&a, &b));
        assert!(check(&b, &b));
        cases += 2;
    }
    assert!(cases > 400);
}

#[test]
fn rich_or_unknown_common_profiles_keep_the_reference_path() {
    let doc = read(
        r#"{"version":"1.0.0","children":[{"type":"rectangle","id":"r","state":{"v":{"type":"object","default":{"z":1,"a":2}}}}]}"#,
    );
    assert_eq!(
        super::exact::geometry_matches(&doc.children[0], &doc.children[0]),
        None
    );
    let plain = read(
        r#"{"version":"1.0.0","children":[{"type":"rectangle","id":"r","width":8,"height":8}]}"#,
    );
    assert_eq!(
        super::exact::geometry_matches(&plain.children[0], &plain.children[0]),
        Some(true)
    );
}

#[test]
fn style_values_inside_one_f32_bucket_remain_distinct_history_edits() {
    let mut before = read(
        r##"{"version":"1.0.0","children":[{"type":"rectangle","id":"r","fill":[{"type":"solid","color":"#123456"}]}]}"##,
    );
    let mut after = before.clone();
    let first = 0.05_f64;
    let second = 0.05000000074505806_f64;
    assert_eq!((first as f32).to_bits(), (second as f32).to_bits());
    assert_ne!(first.to_bits(), second.to_bits());
    set(&mut before, 11, first);
    set(&mut after, 11, second);
    assert!(!check(&before, &after));
}
