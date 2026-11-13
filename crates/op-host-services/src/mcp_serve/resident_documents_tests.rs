use super::super::resident_charge::resident_charge;
use super::*;
use op_editor_core::PenNodeExt;

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "op-resident-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn write(&self, name: &str) -> PathBuf {
        let path = self.0.join(format!("{name}.op"));
        let document = serde_json::json!({"version":"1.0.0","children":[{"type":"rectangle","id":"root","name":name,"width":16,"height":16}]});
        std::fs::write(&path, document.to_string()).unwrap();
        path
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn read_name(lease: &DocumentLease<'_>) -> String {
    lease
        .with_editor(false, |state, _| ActionResult {
            value: state.doc.children[0].base().name.clone().unwrap(),
            persisted: false,
            invalidate: false,
        })
        .unwrap()
}

#[test]
fn invalid_external_replacement_releases_the_stale_editor_before_load_failure() {
    let files = Files::new();
    let path = files.write("old");
    let hub = ResidentDocuments::default();
    let lease = hub.acquire(&path).unwrap();
    assert_eq!(read_name(&lease), "old");
    let invalid = files.0.join("invalid.op");
    std::fs::write(&invalid, b"{invalid external content").unwrap();
    std::fs::rename(invalid, &path).unwrap();
    assert!(lease
        .with_editor(false, |_, _| ActionResult {
            value: (),
            persisted: false,
            invalidate: false,
        })
        .is_err());
    let resident = lease.slot.resident.lock().unwrap();
    assert!(resident.editor.is_none());
    assert!(resident.stamp.is_none());
    assert_eq!(lease.slot.charge.load(Ordering::Relaxed), RESERVATION);
    assert_eq!(std::fs::read(&path).unwrap(), b"{invalid external content");
}

#[test]
fn external_atomic_replacement_invalidates_resident_state_and_conflicting_edits() {
    let files = Files::new();
    let path = files.write("before");
    let hub = ResidentDocuments::default();
    let lease = hub.acquire(&path).unwrap();
    assert_eq!(read_name(&lease), "before");
    let replacement = files.write("external");
    std::fs::rename(replacement, &path).unwrap();
    assert_eq!(read_name(&lease), "external");
    let applied = lease
        .with_editor(false, |state, stamp| {
            state.add_page();
            let replacement = files.write("latest");
            std::fs::rename(replacement, &path).unwrap();
            let conflict = stamp.verify(&path);
            assert!(matches!(
                conflict,
                Err(AuthorityError::ExternalChange { .. })
            ));
            ActionResult {
                value: false,
                persisted: false,
                invalidate: true,
            }
        })
        .unwrap();
    assert!(!applied);
    assert_eq!(read_name(&lease), "latest");
}

#[test]
fn failed_atomic_save_discards_mutated_state_and_keeps_durable_document() {
    let files = Files::new();
    let path = files.write("durable");
    let hub = ResidentDocuments::default();
    let lease = hub.acquire(&path).unwrap();
    let before = std::fs::read(&path).unwrap();
    lease
        .with_editor(false, |state, _| {
            state.doc.children[0].base_mut().name = Some("uncommitted".into());
            let failed =
                super::super::save_editor_state(state, &files.0.join("missing-parent/failed.op"));
            assert!(failed.is_err());
            ActionResult {
                value: (),
                persisted: false,
                invalidate: true,
            }
        })
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(read_name(&lease), "durable");
}

#[test]
fn eviction_is_idle_only_and_count_and_accounting_budgets_are_bounded() {
    let files = Files::new();
    let a = files.write("a");
    let b = files.write("b");
    let c = files.write("c");
    let hub = ResidentDocuments::with_limits(2, 1024 * 1024);
    let first = hub.acquire(&a).unwrap();
    let second = hub.acquire(&b).unwrap();
    assert_eq!(read_name(&first), "a");
    assert_eq!(read_name(&second), "b");
    assert!(matches!(
        hub.acquire(&c),
        Err(AuthorityError::Capacity { .. })
    ));
    let charge = hub.stats().1;
    assert!(charge < 1024 * 1024);
    drop(first);
    let third = hub.acquire(&c).unwrap();
    assert_eq!(read_name(&third), "c");
    assert_eq!(hub.stats().0, 2);
    assert!(hub.stats().1 <= 1024 * 1024);
    assert!(!hub.slots.lock().unwrap().contains_key(&a));
}

#[test]
fn another_file_can_progress_while_one_document_writer_is_busy() {
    let files = Files::new();
    let a = files.write("a");
    let b = files.write("b");
    let hub = ResidentDocuments::default();
    let (entered, receive) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let worker_hub = &hub;
        let worker_path = &a;
        scope.spawn(move || {
            let first = worker_hub.acquire(worker_path).unwrap();
            first
                .with_editor(false, |_, _| {
                    entered.send(()).unwrap();
                    released.recv().unwrap();
                    ActionResult {
                        value: (),
                        persisted: false,
                        invalidate: false,
                    }
                })
                .unwrap();
        });
        receive.recv().unwrap();
        assert_eq!(read_name(&hub.acquire(&b).unwrap()), "b");
        release.send(()).unwrap();
    });
}

#[test]
fn unaccountable_or_over_budget_loaded_states_are_not_retained() {
    let files = Files::new();
    let path = files.write("small");
    let state = super::super::load_editor_state(&path).unwrap();
    let actual = resident_charge(&state, usize::MAX).unwrap();
    let hub = ResidentDocuments::with_limits(2, actual - 1);
    let lease = hub.acquire(&path).unwrap();
    assert!(matches!(
        lease.with_editor(false, |_, _| ActionResult {
            value: (),
            persisted: false,
            invalidate: false
        }),
        Err(AuthorityError::StateOverBudget { .. })
    ));
    assert!(lease.slot.resident.lock().unwrap().editor.is_none());
    assert!(hub.stats().1 <= actual - 1);
}

#[test]
fn concurrent_mcp_clients_edit_two_files_without_lost_updates() {
    let files = Files::new();
    let paths = [files.write("one"), files.write("two")];
    let barrier = std::sync::Barrier::new(16);
    std::thread::scope(|scope| {
        for client in 0..16 {
            let path = &paths[client % 2];
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                for operation in 0..6 {
                    let request = serde_json::json!({"jsonrpc":"2.0","id":client*6+operation,"method":"tools/call","params":{"name":"add_page","arguments":{"filePath":path,"name":format!("client-{client}-{operation}")}}});
                    let response = super::super::file_path::process_message_for_file_path_arg(None, &request.to_string(), super::super::tool_profile::McpAccessProfile::UNRESTRICTED).unwrap().unwrap();
                    assert!(!response.contains("\"isError\":true"), "{response}");
                }
            });
        }
    });
    for path in paths {
        let state = super::super::load_editor_state(&path).unwrap();
        let count = state
            .doc
            .pages
            .as_ref()
            .unwrap()
            .iter()
            .filter(|page| page.name.starts_with("client-"))
            .count();
        assert_eq!(
            count,
            48,
            "every acknowledged write must be durable in {}",
            path.display()
        );
    }
    let (documents, accounted_bytes) = resident_documents().stats();
    eprintln!("{{\"probe\":\"resident-document-authority\",\"logicalClients\":16,\"durableWrites\":96,\"residentDocuments\":{documents},\"accountedStateBytes\":{accounted_bytes}}}");
}

#[test]
fn clipboard_selection_and_undo_redo_survive_interleaved_requests_per_file() {
    let files = Files::new();
    let a = files.write("a");
    let b = files.write("b");
    let call = |path: &Path, tool: &str, extra: serde_json::Value| {
        let mut arguments = extra.as_object().unwrap().clone();
        arguments.insert("filePath".into(), serde_json::json!(path));
        let request = serde_json::json!({"jsonrpc":"2.0","id":59,"method":"tools/call","params":{"name":tool,"arguments":arguments}}).to_string();
        let response = super::super::file_path::process_message_for_file_path_arg(
            None,
            &request,
            super::super::tool_profile::McpAccessProfile::UNRESTRICTED,
        )
        .unwrap()
        .unwrap();
        assert!(!response.contains("\"isError\":true"), "{tool}: {response}");
    };
    for path in [&a, &b] {
        call(path, "set_selection", serde_json::json!({"node_id":"root"}));
        call(path, "copy_selected", serde_json::json!({}));
    }
    call(&a, "paste_clipboard", serde_json::json!({}));
    assert_eq!(
        super::super::load_editor_state(&a)
            .unwrap()
            .doc
            .children
            .len(),
        2
    );
    assert_eq!(
        super::super::load_editor_state(&b)
            .unwrap()
            .doc
            .children
            .len(),
        1
    );
    call(&b, "paste_clipboard", serde_json::json!({}));
    call(&a, "undo", serde_json::json!({}));
    assert_eq!(
        super::super::load_editor_state(&a)
            .unwrap()
            .doc
            .children
            .len(),
        1
    );
    assert_eq!(
        super::super::load_editor_state(&b)
            .unwrap()
            .doc
            .children
            .len(),
        2
    );
    call(&a, "redo", serde_json::json!({}));
    assert_eq!(
        super::super::load_editor_state(&a)
            .unwrap()
            .doc
            .children
            .len(),
        2
    );
}

fn copy_request(source: &Path, destination: &Path) -> String {
    serde_json::json!({"jsonrpc":"2.0","id":55,"method":"tools/call","params":{"name":"save_document","arguments":{"sourceFilePath":source,"filePath":destination}}}).to_string()
}

#[test]
fn opposite_direction_save_copies_complete_without_writer_lock_cycles() {
    let files = Files::new();
    let a = files.write("a");
    let b = files.write("b");
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        for (source, destination) in [(&a, &b), (&b, &a)] {
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                let response = super::super::file_path::process_message_for_file_path_arg(
                    None,
                    &copy_request(source, destination),
                    super::super::tool_profile::McpAccessProfile::UNRESTRICTED,
                )
                .unwrap()
                .unwrap();
                assert!(!response.contains("\"isError\":true"), "{response}");
            });
        }
    });
    let a = super::super::load_editor_state(&a).unwrap();
    let b = super::super::load_editor_state(&b).unwrap();
    assert_eq!(a.doc.children[0].base().name, b.doc.children[0].base().name);
}

#[test]
fn destination_writer_reloads_a_completed_copy_before_applying_its_update() {
    let files = Files::new();
    let source = files.write("copied");
    let destination = files.write("replaced");
    let source_state = super::super::load_editor_state(&source).unwrap();
    let hub = ResidentDocuments::default();
    assert_eq!(read_name(&hub.acquire(&destination).unwrap()), "replaced");
    let (entered, receive) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let worker_hub = &hub;
        let target = &destination;
        scope.spawn(move || {
            worker_hub
                .acquire(target)
                .unwrap()
                .with_destination(|path| {
                    entered.send(()).unwrap();
                    released.recv().unwrap();
                    super::super::save_editor_state(&source_state, path).unwrap();
                    ActionResult {
                        value: (),
                        persisted: true,
                        invalidate: false,
                    }
                })
                .unwrap();
        });
        receive.recv().unwrap();
        let writer = scope.spawn(|| {
            let lease = hub.acquire(&destination).unwrap();
            lease
                .with_editor(false, |state, stamp| {
                    assert_eq!(state.doc.children[0].base().name.as_deref(), Some("copied"));
                    state.doc.children[0].base_mut().name = Some("copied then edited".into());
                    stamp.verify(lease.path()).unwrap();
                    super::super::save_editor_state(state, lease.path()).unwrap();
                    ActionResult {
                        value: (),
                        persisted: true,
                        invalidate: false,
                    }
                })
                .unwrap();
        });
        release.send(()).unwrap();
        writer.join().unwrap();
    });
    assert_eq!(
        read_name(&hub.acquire(&destination).unwrap()),
        "copied then edited"
    );
}

#[test]
fn primary_live_state_is_preserved_and_cross_copy_reports_a_conflict() {
    let files = Files::new();
    let path = files.write("disk");
    let alternate = files.write("alternate");
    let mut primary = super::super::load_editor_state(&path).unwrap();
    primary.doc.children[0].base_mut().name = Some("unsaved live".into());
    let request = serde_json::json!({"jsonrpc":"2.0","id":57,"method":"tools/call","params":{"name":"batch_get","arguments":{"filePath":path,"readDepth":1}}}).to_string();
    let response = super::super::process_message(&mut primary, &path, &request)
        .unwrap()
        .unwrap();
    assert!(response.contains("unsaved live"), "{response}");
    assert!(std::fs::read_to_string(&path).unwrap().contains("disk"));
    for (source, target) in [(&alternate, &path), (&path, &alternate)] {
        let response = super::super::file_path::process_message_for_file_path_arg(
            Some(&path),
            &copy_request(source, target),
            super::super::tool_profile::McpAccessProfile::UNRESTRICTED,
        )
        .unwrap()
        .unwrap();
        assert!(response.contains("live-authority handoff"), "{response}");
    }
    assert_eq!(
        primary.doc.children[0].base().name.as_deref(),
        Some("unsaved live")
    );
}

#[test]
fn live_source_save_uses_current_state_and_invalidates_cached_destination() {
    let files = Files::new();
    let primary_path = files.write("primary disk");
    let destination = files.write("old destination");
    let lease = resident_documents().acquire(&destination).unwrap();
    assert_eq!(read_name(&lease), "old destination");
    let mut primary = super::super::load_editor_state(&primary_path).unwrap();
    primary.doc.children[0].base_mut().name = Some("primary unsaved live".into());
    let response = super::super::process_message(
        &mut primary,
        &primary_path,
        &copy_request(&primary_path, &destination),
    )
    .unwrap()
    .unwrap();
    assert!(!response.contains("\"isError\":true"), "{response}");
    assert_eq!(read_name(&lease), "primary unsaved live");
    assert_eq!(
        primary.doc.children[0].base().name.as_deref(),
        Some("primary unsaved live")
    );
    assert!(std::fs::read_to_string(primary_path)
        .unwrap()
        .contains("primary disk"));
}

#[test]
fn online_and_profile_denials_never_create_or_load_the_target_file() {
    let files = Files::new();
    let missing = files.0.join("not-created/denied.op");
    let request = serde_json::json!({"jsonrpc":"2.0","id":58,"method":"tools/call","params":{"name":"open_document","arguments":{"filePath":missing}}}).to_string();
    for profile in [
        super::super::tool_profile::McpAccessProfile::online(
            super::super::tool_profile::McpScopes::FULL,
        ),
        super::super::tool_profile::McpAccessProfile::LEAN,
    ] {
        let response =
            super::super::file_path::process_message_for_file_path_arg(None, &request, profile)
                .unwrap()
                .unwrap();
        assert!(
            response.contains("not-available") || response.contains("not-in-profile"),
            "{response}"
        );
        assert!(!missing.parent().unwrap().exists());
    }
    let in_memory = request.replace("open_document", "batch_get");
    assert!(super::super::file_path::process_message_for_file_path_arg(
        None,
        &in_memory,
        super::super::tool_profile::McpAccessProfile::online(
            super::super::tool_profile::McpScopes::FULL
        )
    )
    .unwrap()
    .is_none());
}
