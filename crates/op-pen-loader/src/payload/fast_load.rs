//! Existing Preserve contracts exercise the shared semantic fast loader.
use std::sync::Arc;
fn try_load_preserve(
    source: &str,
) -> Option<jian_ops_schema::LoadResult<jian_ops_schema::PenDocument>> {
    if !crate::extract_editor_meta(source)?.preserve_authored_geometry {
        return None;
    }
    super::canonical_stream::try_load_source(source).map(|result| result.loaded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jian_ops_schema::node::PenNode;

    #[test]
    fn fast_preserve_load_resolves_shared_images_and_attaches_thumbnails() {
        let _guard = super::super::lock_thumbnail_registry_for_test();
        let source = "data:image/png;base64,c2hhcmVkLWltYWdl";
        let paint_id = jian_ops_schema::node::image_src::paint_image_id(source);
        let src = format!(
            r#"{{
              "version":"1",
              "children":[
                {{"type":"image","id":"a","src":"op-image:shared"}},
                {{"type":"image","id":"b","src":"op-image:shared"}}
              ],
              "images":{{"shared":"{source}"}},
              "imageThumbs":{{"{paint_id}":"/9j/2Q=="}},
              "editorMeta":{{"activePageIndex":0,"preserveAuthoredGeometry":true}},
              "futureTopLevel":true
            }}"#
        );

        let loaded = try_load_preserve(&src).expect("current Preserve document uses fast load");
        assert_eq!(
            loaded.warnings,
            vec![jian_ops_schema::LoadWarning::UnknownField {
                path: "$".to_owned(),
                field: "futureTopLevel".to_owned(),
            }]
        );
        let (PenNode::Image(first), PenNode::Image(second)) =
            (&loaded.value.children[0], &loaded.value.children[1])
        else {
            panic!("two image nodes expected");
        };
        assert_eq!(first.src.as_str(), source);
        assert_eq!(second.src.as_str(), source);
        assert!(Arc::ptr_eq(&first.src.as_arc(), &second.src.as_arc()));

        assert!(
            jian_ops_schema::image_thumbs::activate_for_document(&loaded.value),
            "the decoded table must stay attached to the typed document"
        );
        assert_eq!(
            &*jian_ops_schema::image_thumbs::thumb_for(paint_id)
                .expect("attached thumbnail activates"),
            &[0xff, 0xd8, 0xff, 0xd9]
        );
    }

    #[test]
    fn legacy_document_bypasses_fast_load_and_keeps_normalization() {
        let src = r##"{
          "version":"2.8",
          "children":[
            {"type":"frame","id":"legacy","fill":"#123456","children":[]}
          ],
          "editorMeta":{"preserveAuthoredGeometry":false}
        }"##;

        assert!(try_load_preserve(src).is_none());
        let loaded = super::super::load_canonical(src).expect("legacy compatibility load");
        assert_eq!(loaded.value.version, "1.0");
        let PenNode::Frame(frame) = &loaded.value.children[0] else {
            panic!("legacy frame expected");
        };
        assert_eq!(frame.container.fill.as_ref().map(Vec::len), Some(1));
    }

    #[test]
    fn preserve_legacy_markers_and_typed_failure_fall_back_to_repair() {
        let src = r##"{
          "version":"1",
          "children":[
            {"type":"frame","id":"legacy","fill":"#abcdef","children":[]},
            {"type":"rectangle","id":"disabled",
             "fill":[{"type":"solid","color":"#ff0000","enabled":false}]},
            {"type":"path","id":"path","geometry":"M0 0L9 9"}
          ],
          "editorMeta":{"preserveAuthoredGeometry":true}
        }"##;

        assert!(try_load_preserve(src).is_none());
        let loaded = super::super::load_canonical(src).expect("fallback repairs legacy fill");
        let PenNode::Frame(frame) = &loaded.value.children[0] else {
            panic!("legacy frame expected");
        };
        assert_eq!(frame.container.fill.as_ref().map(Vec::len), Some(1));
        let PenNode::Rectangle(disabled) = &loaded.value.children[1] else {
            panic!("disabled rectangle expected");
        };
        assert!(
            disabled.container.fill.as_ref().is_some_and(Vec::is_empty),
            "disabled legacy fill must remain unpainted"
        );
        let PenNode::Path(path) = &loaded.value.children[2] else {
            panic!("legacy path expected");
        };
        assert_eq!(path.d.as_deref(), Some("M0 0L9 9"));
    }

    #[test]
    fn escaped_legacy_keys_cannot_bypass_normalization() {
        let src = r##"{
          "version":"1",
          "children":[
            {"type":"rectangle","id":"disabled",
             "fill":[{"type":"solid","color":"#ff0000","en\u0061bled":false}]},
            {"type":"path","id":"path","geo\u006detry":"M0 0L9 9"}
          ],
          "editorMeta":{"preserveAuthoredGeometry":true}
        }"##;

        assert!(try_load_preserve(src).is_none());
        let loaded = super::super::load_canonical(src).expect("escaped keys use legacy repair");
        let PenNode::Rectangle(disabled) = &loaded.value.children[0] else {
            panic!("disabled rectangle expected");
        };
        assert!(disabled.container.fill.as_ref().is_some_and(Vec::is_empty));
        let PenNode::Path(path) = &loaded.value.children[1] else {
            panic!("legacy path expected");
        };
        assert_eq!(path.d.as_deref(), Some("M0 0L9 9"));
    }

    #[test]
    fn preserve_fast_load_rejects_trailing_json_garbage() {
        let src = r#"{
          "version":"1",
          "children":[],
          "editorMeta":{"preserveAuthoredGeometry":true}
        } trailing"#;

        assert!(try_load_preserve(src).is_none());
        assert!(super::super::load_canonical(src).is_err());
    }
}
