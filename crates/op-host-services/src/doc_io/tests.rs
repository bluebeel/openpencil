#[test]
fn html_import_extensions() {
    use std::path::Path;
    assert!(is_supported_html_import(Path::new("a.html")));
    assert!(is_supported_html_import(Path::new("A.HTM")));
    assert!(is_supported_html_import(Path::new("site.zip")));
    assert!(is_supported_html_import(Path::new("SITE.ZIP")));
    assert!(!is_supported_html_import(Path::new("a.svg")));
    assert!(!is_supported_html_import(Path::new("html")));
}

#[test]
fn app_preferences_preserve_runtime_host_capabilities_and_fonts() {
    let mut previous = EditorState::new();
    previous.editor_ui.font_import_supported = true;
    previous
        .editor_ui
        .scene_template_center
        .save_current_supported = true;
    previous.editor_ui.system_fonts_loaded = true;
    previous.editor_ui.system_font_families = std::sync::Arc::new(vec!["PingFang SC".into()]);
    previous.editor_ui.bundled_font_families = std::sync::Arc::new(vec!["Inter".into()]);
    previous.editor_ui.imported_font_families = std::sync::Arc::new(vec!["Brand Sans".into()]);
    let mut next = EditorState::new();

    preserve_app_preferences(&previous, &mut next);

    assert!(next.editor_ui.font_import_supported);
    assert!(
        next.editor_ui.scene_template_center.save_current_supported,
        "Open / New must keep the desktop-only Save As Template row"
    );
    assert!(next.editor_ui.system_fonts_loaded);
    assert_eq!(&*next.editor_ui.system_font_families, &["PingFang SC"]);
    assert_eq!(&*next.editor_ui.bundled_font_families, &["Inter"]);
    assert_eq!(&*next.editor_ui.imported_font_families, &["Brand Sans"]);
}

#[test]
fn entering_a_document_starts_the_chat_panel_minimized() {
    let previous = EditorState::new();
    let mut next = EditorState::new();
    assert!(!next.chat.is_minimized());

    preserve_app_preferences(&previous, &mut next);

    assert!(
        next.chat.is_minimized(),
        "Open / New / template entry starts on the compact bar"
    );
}

#[test]
fn a_streaming_turn_keeps_the_chat_panel_open_across_a_document_swap() {
    let mut previous = EditorState::new();
    previous.chat.set_input_text("design a pricing page");
    assert!(previous.chat.begin_send());
    let mut next = EditorState::new();

    preserve_app_preferences(&previous, &mut next);

    assert!(
        !next.chat.is_minimized(),
        "an in-flight conversation must not be hidden behind the bar"
    );
}

#[test]
fn app_preferences_preserve_prompt_center_store_state() {
    let mut previous = EditorState::new();
    previous
        .editor_ui
        .prompt_center
        .install_custom_prompts(Vec::new(), true);
    previous.editor_ui.prompt_center.add_custom_prompt(
        "Reusable".into(),
        "Reusable body".into(),
        op_editor_core::prompt_center_catalog::PromptCategory::Modify,
        42,
    );
    let mut next = EditorState::new();

    preserve_app_preferences(&previous, &mut next);

    assert_eq!(next.editor_ui.prompt_center.custom_prompts.len(), 1);
    assert!(next.editor_ui.prompt_center.custom_store_writable);
    assert!(next.editor_ui.prompt_center.custom_store_dirty);
}

use super::*;

/// A unique temp path under the OS temp dir for a round-trip test.
fn temp_op_path(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    p.push(format!("openpencil-test-{tag}-{pid}-{nanos}.op"));
    p
}

#[test]
fn active_page_index_survives_a_save_load_round_trip() {
    // Editor view-state (`active_page_index`) is persisted inside
    // the `.op` file so a save / load round-trip restores it
    // without a separate `.opmeta` sidecar.
    let mut state = EditorState::new();
    // Two extra pages → three total; page index 2 is valid.
    state.add_page();
    state.add_page();
    state.ui.active_page_index = 2;

    let path = temp_op_path("page-roundtrip");
    save_to_path(&state, &path).expect("save succeeds");

    let reloaded =
        load_editor_state(&path, op_editor_core::Locale::EnUs).expect("load succeeds");
    assert_eq!(reloaded.ui.active_page_index, 2);
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("saved document"))
            .expect("saved document json");
    assert_eq!(saved["editorMeta"]["activePageIndex"], 2);
    assert_eq!(saved["editorMeta"]["preserveAuthoredGeometry"], false);
    assert!(
        !sidecar_path(&path).exists(),
        "new saves should not create .opmeta sidecars"
    );

    // Cleanup.
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(sidecar_path(&path));
}

#[test]
fn sibling_temp_files_are_create_new_and_unique() {
    let target = temp_op_path("unique-save-temp");
    let (first_path, first_file) = create_sibling_temp(&target).expect("first temp");
    let (second_path, second_file) = create_sibling_temp(&target).expect("second temp");
    assert_ne!(first_path, second_path);
    assert_eq!(first_path.parent(), target.parent());
    assert_eq!(second_path.parent(), target.parent());
    drop(first_file);
    drop(second_file);
    let _ = std::fs::remove_file(first_path);
    let _ = std::fs::remove_file(second_path);
}

#[test]
fn overlapping_saves_use_independent_temps_and_leave_valid_json() {
    let target = temp_op_path("overlapping-saves");
    let mut workers = Vec::new();
    for index in 0..4 {
        let target = target.clone();
        workers.push(std::thread::spawn(move || {
            let mut state = EditorState::new();
            state.doc.name = Some(format!("save-{index}"));
            save_to_path(&state, &target)
        }));
    }
    for worker in workers {
        worker.join().expect("save worker panicked").expect("save");
    }
    let saved: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&target).expect("one complete destination remains"),
    )
    .expect("destination is valid JSON");
    assert!(saved["name"]
        .as_str()
        .is_some_and(|name| name.starts_with("save-")));
    assert!(saved.get("editorMeta").is_some());
    let _ = std::fs::remove_file(&target);
}

#[test]
fn legacy_sidecar_active_page_index_is_still_loaded_and_clamped() {
    // A legacy sidecar that names a page that no longer exists must
    // not leave the editor on an out-of-range index.
    let state = EditorState::new();
    let path = temp_op_path("page-clamp");
    save_to_path(&state, &path).expect("save succeeds");
    // Legacy sidecar from older versions.
    std::fs::write(sidecar_path(&path), r#"{"active_page_index":99}"#)
        .expect("sidecar overwrite");

    let reloaded =
        load_editor_state(&path, op_editor_core::Locale::EnUs).expect("load succeeds");
    // Single-page document → only index 0 is valid.
    assert_eq!(reloaded.ui.active_page_index, 0);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(sidecar_path(&path));
}

#[test]
fn embedded_editor_meta_takes_precedence_over_legacy_sidecar() {
    let path = temp_op_path("embedded-meta");
    std::fs::write(
        &path,
        r#"{"version":"1.0.0","editorMeta":{"activePageIndex":1},"children":[],"pages":[{"id":"p1","name":"One","children":[]},{"id":"p2","name":"Two","children":[]}]}"#,
    )
    .expect("write merged document");
    std::fs::write(sidecar_path(&path), r#"{"active_page_index":0}"#)
        .expect("write stale sidecar");

    let reloaded =
        load_editor_state(&path, op_editor_core::Locale::EnUs).expect("load succeeds");
    assert_eq!(reloaded.ui.active_page_index, 1);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(sidecar_path(&path));
}

#[test]
fn document_fingerprint_is_stable_and_change_sensitive() {
    let state = EditorState::new();
    let fp = document_fingerprint(&state);
    assert_eq!(fp, document_fingerprint(&state), "stable for the same doc");

    let mut mutated = EditorState::new();
    mutated.add_page();
    assert_ne!(
        fp,
        document_fingerprint(&mutated),
        "a structural change moves the fingerprint"
    );
}

#[test]
fn active_page_bbox_resolves_fit_content_root_height() {
    let doc = jian_ops_schema::load_str(
        r#"{
          "version":"1.0.0",
          "children":[{
            "type":"frame", "id":"root", "name":"Explore",
            "x":12, "y":24, "width":390, "height":"fit_content",
            "layout":"vertical",
            "children":[
              {"type":"frame", "id":"header", "width":"fill_container", "height":62},
              {"type":"frame", "id":"content", "width":"fill_container", "height":616},
              {"type":"frame", "id":"tabs", "width":"fill_container", "height":72}
            ]
          }]
        }"#,
    )
    .expect("fixture parses")
    .value;
    let state = EditorState::from_document(doc);

    let (min_x, min_y, max_x, max_y) =
        active_page_bbox(&state).expect("fit-content root has resolved bounds");

    assert!((min_x - 12.0).abs() < 0.01, "min_x={min_x}");
    assert!((min_y - 24.0).abs() < 0.01, "min_y={min_y}");
    assert!((max_x - 402.0).abs() < 0.01, "max_x={max_x}");
    assert!((max_y - 774.0).abs() < 0.01, "max_y={max_y}");
}

#[test]
fn legacy_doc_payload_is_detected() {
    // Fix 4: a pre-canonical private `DocPayload` JSON (integer
    // `version` + `pages` array) is recognised as the legacy
    // format.
    let legacy = r#"{"version":1,"active_page_index":0,"pages":[
        {"id":"n1","name":"Page 1","children":[]}
    ]}"#;
    assert!(looks_like_legacy_doc_payload(legacy));
}

#[test]
fn canonical_document_is_not_mistaken_for_legacy() {
    // A canonical `.op` file has a *string* `version` — it must not
    // trip the legacy detector.
    let canonical = r#"{"version":"1.0.0","children":[]}"#;
    assert!(!looks_like_legacy_doc_payload(canonical));
}

#[test]
fn loading_a_legacy_file_surfaces_an_explicit_error() {
    // The load path turns the legacy format into an actionable
    // user-facing message rather than an opaque schema error.
    let path = temp_op_path("legacy-load");
    std::fs::write(
        &path,
        r#"{"version":1,"active_page_index":0,"pages":[
            {"id":"n1","name":"Page 1","children":[]}
        ]}"#,
    )
    .expect("write legacy fixture");

    let err = load_editor_state(&path, op_editor_core::Locale::EnUs)
        .expect_err("legacy file is rejected");
    // The legacy detector surfaces the `dialog.loadErrorOldVersion`
    // localised message rather than an opaque schema error.
    assert_eq!(
        err.to_string(),
        op_i18n::translate(op_editor_core::Locale::EnUs, "dialog.loadErrorOldVersion")
    );

    let _ = std::fs::remove_file(&path);
}

/// A multi-page document whose FIRST page is an empty cover opened onto
/// plain canvas ("content bbox None") and read as a broken file — the
/// content was on page 2 (measured: zwiki-ui-states.op, 16 pages).
#[test]
fn a_file_with_an_empty_first_page_opens_on_the_first_page_with_content() {
    let path = temp_op_path("empty-first-page");
    std::fs::write(
        &path,
        r##"{ "version": "1.0", "children": [], "pages": [
            { "id": "p1", "name": "Cover", "children": [] },
            { "id": "p2", "name": "States", "children": [
                { "type": "frame", "id": "n1", "name": "Screen",
                  "width": 390, "height": 844 }
            ]}
        ]}"##,
    )
    .expect("write");
    let state = load_editor_state(&path, op_editor_core::Locale::EnUs).expect("loads");
    assert_eq!(
        state.ui.active_page_index, 1,
        "the blank cover is skipped — the file opens where the design is"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_file_whose_first_page_has_content_still_opens_on_it() {
    let path = temp_op_path("first-page-content");
    std::fs::write(
        &path,
        r##"{ "version": "1.0", "children": [], "pages": [
            { "id": "p1", "name": "Home", "children": [
                { "type": "frame", "id": "n1", "width": 390, "height": 844 }
            ]},
            { "id": "p2", "name": "Detail", "children": [
                { "type": "frame", "id": "n2", "width": 390, "height": 844 }
            ]}
        ]}"##,
    )
    .expect("write");
    let state = load_editor_state(&path, op_editor_core::Locale::EnUs).expect("loads");
    assert_eq!(state.ui.active_page_index, 0);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn an_entirely_empty_file_still_lands_on_page_zero() {
    let path = temp_op_path("all-empty");
    std::fs::write(
        &path,
        r##"{ "version": "1.0", "children": [], "pages": [
            { "id": "p1", "name": "A", "children": [] },
            { "id": "p2", "name": "B", "children": [] }
        ]}"##,
    )
    .expect("write");
    let state = load_editor_state(&path, op_editor_core::Locale::EnUs).expect("loads");
    assert_eq!(state.ui.active_page_index, 0);
    let _ = std::fs::remove_file(&path);
}
