use super::*;
use serde_json::{json, Value};
fn compare(source: &str, fast: bool) {
    assert_eq!(try_load_source(source).is_some(), fast, "{source}");
    let reference = super::super::load_canonical_dom_with_compatibility(source).unwrap();
    let actual = super::super::load_canonical_with_compatibility(source).unwrap();
    assert_eq!(
        serde_json::to_vec(&actual.loaded.value).unwrap(),
        serde_json::to_vec(&reference.loaded.value).unwrap()
    );
    assert_eq!(actual.loaded.warnings, reference.loaded.warnings);
    assert_eq!(actual.compatibility, reference.compatibility);
}
#[test]
fn structural_probe_keeps_canonical_enabled_text_variables_and_rich_paints_fast() {
    compare(
        r##"{"version":"1","variables":{"enabled":{"type":"color","value":"#123456"}},"children":[{"type":"text","id":"t","content":"enabled geometry video","enabled":false,"fill":[{"type":"solid","color":"#123456"}],"gestures":{"future":{"geometry":"opaque","enabled":true}},"events":{"future":{"type":"color","value":1}}}]}"##,
        true,
    );
    compare(
        r##"{"version":"1","children":[{"type":"text","id":"t","content":[{"text":"rich","fill":"#abcdef"}]}]}"##,
        true,
    );
}
#[test]
fn streamed_header_matches_unknown_warning_order_logic_modules_and_future_minor() {
    compare(
        r#"{"version":"1","formatVersion":"1.3","futureZ":{"deep":[1,2]},"futureA":true,"logicModules":[],"children":[]}"#,
        true,
    );
}
#[test]
fn sparse_metadata_keeps_none_empty_rich_unknown_payloads_and_negative_zero() {
    for metadata in [
        Value::Null,
        json!({}),
        json!({"onTap":[{"futureAction":{"x":-0.0}}],"futureHandler":{"payload":[1,true,"enabled"]}}),
    ] {
        let mut node = json!({"type":"frame","id":"r","x":-0.0,"children":[{"type":"text","id":"t","content":"text"}]});
        if !metadata.is_null() {
            node["events"] = metadata;
            node["lifecycle"] = json!({});
            node["semantics"] = json!({});
            node["gestures"] = json!({"future":{"arbitrary":[1,2]}})
        }
        let source = json!({"version":"1","children":[node]}).to_string();
        compare(&source, true);
        let loaded = try_load_source(&source).unwrap();
        let jian_ops_schema::node::PenNode::Frame(node) = &loaded.loaded.value.children[0] else {
            panic!()
        };
        assert_eq!(node.base.x.unwrap().to_bits(), (-0.0_f64).to_bits());
    }
    compare(
        r#"{"version":"1","children":[{"type":"number_input","id":"n","events":{},"lifecycle":{},"semantics":{},"gestures":{},"states":{"hover":{"opacity":-0.0}}}]}"#,
        true,
    );
}
#[test]
fn legacy_shapes_in_opaque_metadata_cannot_bypass_preprocessing() {
    compare(&json!({"version":"1","children":[{"type":"rectangle","id":"r","fill":[{"type":"solid","color":"#fff","enabled":false}]}]}).to_string(), true);
    for node in [
        json!({"type":"path","id":"p","geometry":"M0 0L1 1"}),
        json!({"type":"frame","id":"f","cornerRadius":"missing-variable"}),
        json!({"type":"frame","id":"f","padding":["missing-variable",4]}),
        json!({"type":"text","id":"t","content":"x","events":{"future":{"type":"video","src":"movie","poster":"poster"}}}),
        json!({"type":"frame","id":"f","events":{"future":{"type":"path","id":"opaque","geometry":"M0 0"}}}),
        json!({"type":"frame","id":"f","events":{"future":{"type":"rectangle","id":"opaque","stroke":"#123456"}}}),
    ] {
        compare(&json!({"version":"1","children":[node]}).to_string(), false)
    }
    compare(
        r#"{"version":"1","children":[{"type":"path","id":"p","geo\u006detry":"M0 0"}]}"#,
        false,
    );
}
#[test]
fn duplicate_fields_and_responsive_documents_retain_dom_contracts() {
    compare(
        r#"{"version":"1","children":[{"type":"rectangle","id":"r","x":1,"x":2}]}"#,
        false,
    );
    compare(
        r#"{"version":"1","children":[],"children":[{"type":"rectangle","id":"r"}]}"#,
        false,
    );
    compare(r#"{"version":"1","responsive":true,"children":[]}"#, false);
    compare(
        r#"{"version":"1","editorMeta":{"activePageIndex":0},"children":[]}"#,
        false,
    );
}
#[test]
fn malformed_and_deep_streams_keep_the_established_fallback() {
    for source in [
        r#"{"version":"1","future":{"broken":[1,] },"children":[]}"#,
        r#"{"version":"1","children":[]} trailing"#,
    ] {
        assert!(try_load_source(source).is_none());
        assert!(super::super::load_canonical_with_compatibility(source).is_err())
    }
    let source = format!(
        "{}{}{}",
        r#"{"type":"frame","id":"deep","children":["#.repeat(140),
        r#"{"type":"rectangle","id":"leaf"}"#,
        "]}".repeat(140)
    );
    let source = format!(r#"{{"version":"1","children":[{source}]}}"#);
    compare(&source, false);
}
#[cfg(not(target_arch = "wasm32"))]
mod files {
    use super::*;
    use std::{
        fs::{self, File},
        path::PathBuf,
    };
    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "op-stream-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&p).unwrap();
        p
    }
    #[test]
    fn file_loading_uses_open_inode_and_preserves_editor_metadata() {
        let dir = temp("inode");
        let path = dir.join("doc.op");
        let source = r#"{"version":"1","pages":[{"id":"p0","name":"A","children":[]},{"id":"p1","name":"B","children":[]}],"editorMeta":{"activePageIndex":1,"preserveAuthoredGeometry":false}}"#;
        fs::write(&path, source).unwrap();
        let file = File::open(&path).unwrap();
        let new = dir.join("new.op");
        fs::write(
            &new,
            r#"{"version":"1","children":[{"type":"text","id":"new","content":"replacement"}]}"#,
        )
        .unwrap();
        fs::rename(new, &path).unwrap();
        let actual = load_canonical_file_with_compatibility(&file).unwrap();
        assert_eq!(
            actual.canonical.loaded.value.pages.as_ref().unwrap().len(),
            2
        );
        let meta = actual.editor_meta.unwrap();
        assert_eq!(meta.meta.active_page_index, 1);
        assert!(!meta.meta.preserve_authored_geometry);
        assert_eq!(
            serde_json::to_vec(&actual.canonical.loaded.value).unwrap(),
            serde_json::to_vec(
                &super::super::super::load_canonical_dom_with_compatibility(source)
                    .unwrap()
                    .loaded
                    .value
            )
            .unwrap()
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn file_errors_keep_utf8_empty_and_legacy_format_classification() {
        let dir = temp("errors");
        let path = dir.join("doc.op");
        fs::write(&path, [0xff_u8]).unwrap();
        assert!(matches!(
            load_canonical_file_with_compatibility(&File::open(&path).unwrap()),
            Err(CanonicalFileLoadError::InvalidUtf8(_))
        ));
        fs::write(&path, []).unwrap();
        assert!(matches!(
            load_canonical_file_with_compatibility(&File::open(&path).unwrap()),
            Err(CanonicalFileLoadError::Schema {
                legacy_doc_payload: false,
                ..
            })
        ));
        fs::write(&path, r#"{"version":7,"pages":[{"nodes":[]}]}"#).unwrap();
        assert!(matches!(
            load_canonical_file_with_compatibility(&File::open(&path).unwrap()),
            Err(CanonicalFileLoadError::Schema {
                legacy_doc_payload: true,
                ..
            })
        ));
        fs::remove_dir_all(dir).unwrap();
    }
}
