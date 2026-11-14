use super::*;

fn temp() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "op-headless-fonts-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn absent_configuration_keeps_fallback_without_loading_files() {
    for value in [None, Some(OsStr::new(""))] {
        assert_eq!(
            load(value, |_, _| panic!("unconfigured fonts must not register")).unwrap(),
            0
        );
    }
}

#[test]
fn explicit_paths_keep_order_and_bytes_and_deduplicate_aliases() {
    let root = temp();
    let first = root.join("first α.sfnt");
    let second = root.join("second.otf");
    std::fs::write(&first, b"exact-first").unwrap();
    std::fs::write(&second, b"exact-second").unwrap();
    let paths = std::env::join_paths([
        first.clone(),
        root.join(".").join("first α.sfnt"),
        second.clone(),
    ])
    .unwrap();
    let mut loaded = Vec::new();
    assert_eq!(
        load(Some(&paths), |path, bytes| {
            loaded.push((path.to_path_buf(), bytes));
            Ok(())
        })
        .unwrap(),
        2
    );
    assert_eq!(
        loaded,
        vec![
            (first.canonicalize().unwrap(), b"exact-first".to_vec()),
            (second.canonicalize().unwrap(), b"exact-second".to_vec())
        ]
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unreadable_input_fails_before_any_registration() {
    let root = temp();
    let valid = root.join("first.sfnt");
    std::fs::write(&valid, b"font").unwrap();
    let paths = std::env::join_paths([valid, root.join("missing.sfnt")]).unwrap();
    assert!(matches!(
        load(Some(&paths), |_, _| panic!(
            "preflight must finish before registration"
        )),
        Err(FontStartupError::Io { .. })
    ));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn invalid_font_reports_its_path_instead_of_silent_fallback() {
    let root = temp();
    let path = root.join("invalid.sfnt");
    std::fs::write(&path, b"not a font").unwrap();
    let paths = std::env::join_paths([path.clone()]).unwrap();
    let result = load(Some(&paths), |path, bytes| {
        jian_skia::register_imported_font(bytes)
            .map(|_| ())
            .map_err(|reason| FontStartupError::Invalid {
                path: path.to_path_buf(),
                reason,
            })
    });
    assert!(
        matches!(result, Err(FontStartupError::Invalid { path: actual, .. }) if actual == path.canonicalize().unwrap())
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn oversized_file_is_bounded_before_registration() {
    let root = temp();
    let path = root.join("large.sfnt");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_BYTES as u64 + 1).unwrap();
    let paths = std::env::join_paths([path]).unwrap();
    assert!(matches!(
        load(Some(&paths), |_, _| panic!(
            "over-budget fonts must not register"
        )),
        Err(FontStartupError::Capacity { .. })
    ));
    std::fs::remove_dir_all(root).unwrap();
}
