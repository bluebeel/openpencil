//! Exact JSON precision through the same load/save paths used by MCP hosts.
use crate::doc_io::{load_editor_state, load_editor_state_from_source, save_to_path};
use op_editor_core::Locale;
use serde_json::Value;
use std::path::PathBuf;

const REPORTED: [&str; 4] = [
    "11.899999618530273",
    "0.46000000834465027",
    "15.833333015441895",
    "209.52000427246094",
];

#[test]
fn float_roundtrip_typed_and_value_decoders_retain_reported_bits() {
    for source in REPORTED.into_iter().chain(["-0.0", "5e-324"]) {
        let expected = source.parse::<f64>().expect("valid finite decimal");
        let typed: f64 = serde_json::from_str(source).expect("typed JSON number");
        let value: Value = serde_json::from_str(source).expect("dynamic JSON number");
        assert_eq!(typed.to_bits(), expected.to_bits(), "typed {source}");
        assert_eq!(
            value.as_f64().expect("number").to_bits(),
            expected.to_bits(),
            "Value {source}"
        );
    }
}

struct TemporaryDocument(PathBuf);

impl TemporaryDocument {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "openpencil-float-roundtrip-{}-{}.op",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        )))
    }
}

impl Drop for TemporaryDocument {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn precision_document() -> String {
    // A bounded, public numeric fixture reproduces the 1,852 unrelated
    // precision properties observed in a real imported document, without
    // embedding that proprietary document or normalizing away its numbers.
    let nodes = (0..463)
        .map(|index| {
            format!(
                r#"{{"type":"rectangle","id":"precision-{index}","x":{},"y":{},"width":{},"height":{}}}"#,
                REPORTED[0], REPORTED[1], REPORTED[2], REPORTED[3]
            )
        })
        .chain(std::iter::once(
            r#"{"type":"rectangle","id":"signed-zero","x":-0.0,"y":0.0,"width":4,"height":4}"#.to_string(),
        ))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"{{"version":"1.0.0","children":[],"editorMeta":{{"activePageIndex":0,"preserveAuthoredGeometry":true}},"pages":[{{"id":"precision-page","name":"Precision","children":[{nodes}]}}]}}"#
    )
}

fn assert_document_bits(document: &jian_ops_schema::PenDocument) {
    let value = serde_json::to_value(document).expect("typed document to Value");
    let nodes = value["pages"][0]["children"]
        .as_array()
        .expect("page children");
    assert_eq!(nodes.len(), 464);
    for (index, node) in nodes[..463].iter().enumerate() {
        assert_eq!(node["id"], format!("precision-{index}"));
        for (field, source) in ["x", "y", "width", "height"].into_iter().zip(REPORTED) {
            let expected = source.parse::<f64>().expect("finite decimal");
            assert_eq!(
                node[field].as_f64().expect("numeric property").to_bits(),
                expected.to_bits(),
                "node {index} {field}: no tolerance or f32 narrowing"
            );
        }
    }
    assert_eq!(nodes[463]["id"], "signed-zero");
    assert_eq!(
        nodes[463]["x"].as_f64().unwrap().to_bits(),
        (-0.0_f64).to_bits()
    );
    assert_eq!(
        nodes[463]["y"].as_f64().unwrap().to_bits(),
        0.0_f64.to_bits()
    );
}

#[test]
fn float_roundtrip_production_document_load_save_retains_1852_properties() {
    let source = precision_document();
    let state = load_editor_state_from_source(&source, Locale::EnUs).expect("source load");
    assert_document_bits(&state.doc);
    assert!(state.editor_ui.preserve_authored_geometry);

    let file = TemporaryDocument::new();
    save_to_path(&state, &file.0).expect("production atomic save");
    let saved = std::fs::read_to_string(&file.0).expect("saved canonical source");
    let saved_value: Value = serde_json::from_str(&saved).expect("saved dynamic JSON");
    assert_eq!(saved_value["editorMeta"]["activePageIndex"], 0);
    assert_eq!(saved_value["editorMeta"]["preserveAuthoredGeometry"], true);
    for reopened in [
        load_editor_state(&file.0, Locale::EnUs).expect("mapped production file load"),
        load_editor_state_from_source(&saved, Locale::EnUs).expect("saved source load"),
    ] {
        assert_document_bits(&reopened.doc);
        assert!(reopened.editor_ui.preserve_authored_geometry);
    }
}
