use super::*;

#[test]
fn mcp_bulk_raster_file_format_matches_requested_encoder() {
    let state = EditorState::from_document(jian_ops_schema::load_str(
        r##"{"version":"1.0.0","children":[{"id":"board","type":"frame","name":"Board.with.png","width":32,"height":24,"fill":[{"type":"solid","color":"#1234ab80"}]}]}"##,
    ).unwrap().value);
    let tool = export_frames_snapshot(&state);
    let directory = std::env::temp_dir().join(format!("op-bulk-formats-{}", std::process::id()));
    let formats = ["png", "jpeg", "jpg"].into_iter().chain(
        cfg!(any(
            target_os = "macos",
            target_os = "linux",
            target_os = "windows"
        ))
        .then_some("webp"),
    );
    for format in formats {
        let output = directory.join(format);
        let args = BTreeMap::from([
            ("format".into(), format.into()),
            ("outputDir".into(), output.to_str().unwrap().into()),
        ]);
        let ToolOutcome::OkJson(json) = tool.call(&args) else {
            panic!("bulk {format} rejected")
        };
        let report: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(report["format"], format);
        assert_eq!(report["attempted"], 1);
        assert_eq!(report["failed"].as_array().unwrap().len(), 0, "{report}");
        let name = report["written"][0].as_str().unwrap();
        assert_eq!(
            name,
            op_editor_core::export_batch::plan_frame_exports(&state, format)[0].file_name
        );
        assert!(name.ends_with(&format!(".{format}")));
        let bytes = std::fs::read(output.join(name)).unwrap();
        match format {
            "png" => assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n")),
            "jpeg" | "jpg" => assert!(bytes.starts_with(&[0xff, 0xd8, 0xff])),
            "webp" => {
                assert_eq!(&bytes[..4], b"RIFF");
                assert_eq!(&bytes[8..12], b"WEBP");
            }
            _ => unreachable!(),
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn board_query_snapshot_is_complete_after_original_state_is_dropped() {
    let state = EditorState::from_document(jian_ops_schema::load_str(
        r##"{"version":"1.0.0","children":[{"id":"board","type":"frame","name":"Board","width":32,"height":24}]}"##,
    ).unwrap().value);
    let boards = op_editor_core::preview_slideshow::active_page_boards(&state);
    let expected = serde_json::json!({ "count": boards.len(), "boards": boards }).to_string();
    let snapshot = get_deck_boards_snapshot(&state);
    drop(state);
    let ToolOutcome::OkJson(actual) = snapshot.call(&BTreeMap::new()) else {
        panic!("board query rejected")
    };
    assert_eq!(actual, expected);
}
