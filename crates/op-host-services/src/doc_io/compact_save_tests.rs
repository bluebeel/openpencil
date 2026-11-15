use super::*;
use op_editor_core::PenNodeExt;
use serde_json::{json, Value};

struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("op-compact-{}-{unique}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn state() -> EditorState {
    let mut state = EditorState::from_document(
        serde_json::from_value(json!({"version":"1.0.0","children":[
            {"type":"frame","id":"absent","x":11.899999618530273,"y":-0.0},
            {"type":"frame","id":"empty","children":[]}
        ]}))
        .unwrap(),
    );
    state.editor_ui.preserve_authored_geometry = true;
    state
}

#[test]
fn compact_atomic_save_matches_explicit_save_and_round_trips_editor_metadata() {
    let files = Files::new();
    let pretty = files.0.join("pretty.op");
    let compact = files.0.join("compact.op");
    let state = state();
    std::fs::write(sidecar_path(&compact), "stale").unwrap();
    save_to_path(&state, &pretty).unwrap();
    save_to_path_compact(&state, &compact).unwrap();
    let pretty_bytes = std::fs::read(&pretty).unwrap();
    let compact_bytes = std::fs::read(&compact).unwrap();
    let expected: Value = serde_json::from_slice(&pretty_bytes).unwrap();
    assert_eq!(compact_bytes, serde_json::to_vec(&expected).unwrap());
    assert!(!sidecar_path(&compact).exists());
    let restored = load_editor_state(&compact, op_editor_core::Locale::EnUs).unwrap();
    assert!(restored.editor_ui.preserve_authored_geometry);
    assert_eq!(
        restored.doc.children[0].base().x.unwrap().to_bits(),
        state.doc.children[0].base().x.unwrap().to_bits()
    );
    assert_eq!(
        restored.doc.children[0].base().y.unwrap().to_bits(),
        (-0.0f64).to_bits()
    );
    assert!(restored.doc.children[0].children().is_none());
    assert_eq!(restored.doc.children[1].children().unwrap().len(), 0);
}

#[test]
fn compact_atomic_save_failure_keeps_destination_and_cleans_staging_file() {
    let files = Files::new();
    let destination = files.0.join("blocked.op");
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(sidecar_path(&destination), "keep").unwrap();
    let before = std::fs::read_dir(&files.0).unwrap().count();
    assert!(save_to_path_compact(&state(), &destination).is_err());
    assert!(destination.is_dir());
    assert_eq!(
        std::fs::read_to_string(sidecar_path(&destination)).unwrap(),
        "keep"
    );
    assert_eq!(std::fs::read_dir(&files.0).unwrap().count(), before);
}
