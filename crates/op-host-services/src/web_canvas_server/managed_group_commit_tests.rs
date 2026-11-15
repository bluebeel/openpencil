//! Prepared full-host controls; root runs these with the target-capture16 API.
use super::*;
use crate::web_canvas_server::{connection::ConnCtx, ConnAuthority, SseHub};
use op_editor_core::{EditorState, NodeId, PenNodeExt, SelectionState};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
const PROFILE: McpAccessProfile = McpAccessProfile::UNRESTRICTED;

struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "op-managed-group-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> PathBuf {
        self.0.join("primary.op")
    }
    fn state(&self, blocked: bool) -> Mutex<WebCanvasState> {
        let document = serde_json::from_value(json!({"version":"1.0.0","children":[],"pages":[
            {"id":"p0","name":"one","children":[
                {"type":"rectangle","id":"human","width":10,"height":10},
                {"type":"rectangle","id":"n0","x":0,"width":10,"height":10},
                {"type":"rectangle","id":"n2","x":0,"width":10,"height":10}]},
            {"id":"p1","name":"two","children":[
                {"type":"rectangle","id":"n1","x":0,"width":10,"height":10},
                {"type":"rectangle","id":"n3","x":0,"width":10,"height":10}]}]}))
        .unwrap();
        let mut editor = EditorState::from_document(document);
        editor.editor_ui.preserve_authored_geometry = true;
        editor.selection = SelectionState {
            anchor: NodeId::new("human"),
            set: vec![NodeId::new("human")],
        };
        editor.viewport.pan_x = 123.0;
        if blocked {
            std::fs::create_dir(self.path()).unwrap();
            std::fs::write(self.path().join("keep"), b"prior authority").unwrap();
        } else {
            crate::doc_io::save_to_path_compact(&mut editor, &self.path()).unwrap();
        }
        let mut state = WebCanvasState::new_with_path(editor, 3100, Some(self.path()));
        state.mode = ServeMode::Managed;
        Mutex::new(state)
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn line(index: usize, value: i32) -> String {
    json!({"jsonrpc":"2.0","id":index,"method":"tools/call","params":{
        "name":"update_node","arguments":{"agentId":format!("agent{index}"),
        "nodeId":format!("n{index}"),"pageId":format!("p{}",index % 2),"data":{"x":value}}}})
    .to_string()
}

fn ctx<'a>(
    state: &'a Mutex<WebCanvasState>,
    authority: &'a ConnAuthority,
    hub: &'a SseHub,
) -> ConnCtx<'a> {
    ConnCtx {
        state,
        authority,
        hub,
        mode: ServeMode::Managed,
        mcp_profile: PROFILE,
        rest_identity: None,
        write_barrier: None,
    }
}

fn queued<'a>(ctx: &ConnCtx<'a>, lines: &[String]) -> Vec<Value> {
    let admissions: Vec<_> = lines
        .iter()
        .map(|line| {
            ctx.authority
                .agent_contexts
                .try_admit(line, PROFILE)
                .unwrap()
        })
        .collect();
    let (path, recovery_epoch, generation) = {
        let state = ctx.state.lock().unwrap();
        (
            state.current_path.clone().unwrap(),
            state.recovery.epoch(),
            state.editor.document_generation(),
        )
    };
    // Install each actor's actual UI context before moving execution to the
    // leader. The original admission tokens remain alive until every reply.
    for (index, line) in lines.iter().enumerate() {
        let mut state = ctx.state.lock().unwrap();
        mcp_serve::agent_context::with_agent_context(
            &ctx.authority.agent_contexts,
            &path,
            &mut state.editor,
            line,
            PROFILE,
            |editor| {
                editor.ui.active_page_index = index % 2;
                editor.viewport.pan_x = index as f32 + 700.0;
                Ok(None)
            },
        )
        .unwrap();
    }
    let groups = &ctx.authority.managed_groups;
    let tickets: Vec<_> = lines
        .iter()
        .zip(&admissions)
        .map(|(line, admission)| {
            groups
                .queue
                .submit(
                    Request {
                        line: line.clone(),
                        structural: op_mcp::parse_tool_call(line).is_some_and(|call| {
                            mcp_serve::cohort_coverage::structural_tool(&call.tool)
                        }),
                        profile: PROFILE,
                        path: path.clone(),
                        document: path.clone(),
                        state_identity: ctx.state as *const _ as usize,
                        recovery_epoch,
                        generation,
                        permit: admission
                            .execution_permit(&ctx.authority.agent_contexts, &path)
                            .unwrap(),
                    },
                    line.len(),
                    line.len() + MAX_REPLY + 2048,
                )
                .ok()
                .unwrap()
        })
        .collect();
    let result = std::thread::scope(|scope| {
        let handles: Vec<_> = tickets
            .into_iter()
            .map(|ticket| {
                scope.spawn(move || {
                    match ticket.execute(
                        |requests| run_group(ctx, groups, requests),
                        |request| {
                            Reply::Complete(Ok(error(&request.line, &GroupError::LeaderPanicked)))
                        },
                    ) {
                        Reply::Complete(response) => {
                            serde_json::from_str(&response.unwrap()).unwrap()
                        }
                        Reply::Isolated => panic!("expected supported target-only group"),
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });
    drop(admissions);
    result
}

#[test]
fn four_writers_save_once_keep_separate_undo_steps_and_final_actor_metadata() {
    let files = Files::new();
    let state = files.state(false);
    let authority = ConnAuthority::new(&state.lock().unwrap());
    let hub = SseHub::default();
    let ctx = ctx(&state, &authority, &hub);
    let lines: Vec<_> = (0..4).map(|n| line(n, n as i32 + 10)).collect();
    let replies = queued(&ctx, &lines);
    assert!(replies
        .iter()
        .all(|reply| reply["result"]["isError"] != true));
    assert_eq!(authority.managed_groups.saves.load(Ordering::Acquire), 1);
    let saved: Value = serde_json::from_slice(&std::fs::read(files.path()).unwrap()).unwrap();
    assert_eq!(saved["editorMeta"]["activePageIndex"], 1);
    let mut state = state.lock().unwrap();
    assert_eq!(state.version, 4);
    assert_eq!(state.editor.history.past.len(), 4);
    assert_eq!(state.editor.ui.active_page_index, 0);
    assert_eq!(state.editor.selection.anchor.as_str(), "human");
    assert_eq!(state.editor.viewport.pan_x, 123.0);
    for index in (0..4).rev() {
        assert!(state.editor.undo());
        let node = op_editor_core::walkers::find_node(
            &state.editor.doc.pages.as_ref().unwrap()[index % 2].children,
            &NodeId::new(format!("n{index}")),
        )
        .unwrap();
        assert_eq!(node.base().x, Some(0.0));
    }
}

#[test]
fn failed_save_rolls_back_every_actor_and_preserves_the_live_editor() {
    let files = Files::new();
    let state = files.state(true);
    let before = serde_json::to_vec(&state.lock().unwrap().editor.doc).unwrap();
    let authority = ConnAuthority::new(&state.lock().unwrap());
    let hub = SseHub::default();
    let replies = queued(
        &ctx(&state, &authority, &hub),
        &(0..4).map(|n| line(n, 42)).collect::<Vec<_>>(),
    );
    assert!(replies
        .iter()
        .all(|reply| reply["result"]["isError"] == true));
    assert_eq!(authority.managed_groups.saves.load(Ordering::Acquire), 0);
    let state = state.lock().unwrap();
    assert_eq!(serde_json::to_vec(&state.editor.doc).unwrap(), before);
    assert_eq!(state.version, 0);
    assert!(state.editor.history.past.is_empty());
    assert_eq!(state.editor.selection.anchor.as_str(), "human");
    assert_eq!(state.editor.ui.active_page_index, 0);
    assert_eq!(state.editor.viewport.pan_x, 123.0);
    assert_eq!(
        std::fs::read(files.path().join("keep")).unwrap(),
        b"prior authority"
    );
}

#[test]
fn a_noop_depending_on_an_uncommitted_writer_cannot_ack_after_failed_save() {
    let files = Files::new();
    let state = files.state(true);
    let authority = ConnAuthority::new(&state.lock().unwrap());
    let hub = SseHub::default();
    let first = line(0, 9);
    let mut second: Value = serde_json::from_str(&first).unwrap();
    second["id"] = 1.into();
    second["params"]["arguments"]["agentId"] = "other".into();
    let replies = queued(&ctx(&state, &authority, &hub), &[first, second.to_string()]);
    assert!(replies
        .iter()
        .all(|reply| reply["result"]["isError"] == true));
    let state = state.lock().unwrap();
    assert_eq!(
        state.editor.doc.pages.as_ref().unwrap()[0].children[1]
            .base()
            .x,
        Some(0.0)
    );
}

#[test]
fn structural_patch_falls_back_before_any_command_is_applied() {
    let files = Files::new();
    let state = files.state(false);
    let authority = ConnAuthority::new(&state.lock().unwrap());
    let hub = SseHub::default();
    let ctx = ctx(&state, &authority, &hub);
    let mut input: Value = serde_json::from_str(&line(0, 1)).unwrap();
    input["params"]["arguments"]["data"] = json!({"children":[]});
    let input = input.to_string();
    let admission = authority.agent_contexts.try_admit(&input, PROFILE).unwrap();
    let before = serde_json::to_vec(&state.lock().unwrap().editor.doc).unwrap();
    assert!(dispatch(
        &ctx,
        &input,
        PROFILE,
        Some(&files.path()),
        &files.path(),
        &admission,
        true
    )
    .is_none());
    assert_eq!(
        serde_json::to_vec(&state.lock().unwrap().editor.doc).unwrap(),
        before
    );
    assert_eq!(authority.managed_groups.saves.load(Ordering::Acquire), 0);
}

#[test]
fn large_authored_text_uses_the_total_capture_allowance() {
    let files = Files::new();
    let state = files.state(false);
    let mut guard = state.lock().unwrap();
    guard.editor.doc.pages.as_mut().unwrap()[0].children[1] = serde_json::from_value(json!({
        "type":"text","id":"n0","content":"x".repeat(2 * 1024 * 1024)}))
    .unwrap();
    drop(guard);
    let authority = ConnAuthority::new(&state.lock().unwrap());
    let hub = SseHub::default();
    let input = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
        "name":"set_node_text","arguments":{"agentId":"a","node_id":"n0","text":"small"}}})
    .to_string();
    let admission = authority.agent_contexts.try_admit(&input, PROFILE).unwrap();
    let before = state.lock().unwrap().editor.document_revision();
    let response = dispatch(
        &ctx(&state, &authority, &hub),
        &input,
        PROFILE,
        Some(&files.path()),
        &files.path(),
        &admission,
        true,
    )
    .expect("large text fits the approved total allowance")
    .unwrap();
    let response: Value = serde_json::from_str(&response).unwrap();
    assert!(response.get("error").is_none() && response["result"]["isError"] != true);
    let state = state.lock().unwrap();
    assert!(state.editor.document_revision() > before);
    assert_eq!(state.editor.history.past.len(), 1);
    assert_eq!(
        serde_json::to_value(&state.editor.doc).unwrap()["pages"][0]["children"][1]["content"],
        "small"
    );
}

#[test]
fn queued_permit_pins_admission_after_the_original_waiter_is_canceled() {
    let files = Files::new();
    let state = files.state(false);
    let authority = ConnAuthority::new(&state.lock().unwrap());
    let input = line(0, 1);
    let admission = authority.agent_contexts.try_admit(&input, PROFILE).unwrap();
    let permit = admission
        .execution_permit(&authority.agent_contexts, &files.path())
        .unwrap();
    drop(admission);
    assert!(authority.agent_contexts.try_admit(&input, PROFILE).is_err());
    let mut guard = state.lock().unwrap();
    let mut called = false;
    let response = mcp_serve::agent_context::with_agent_context_permit(
        &authority.agent_contexts,
        &files.path(),
        &mut guard.editor,
        &input,
        PROFILE,
        &permit,
        |_| {
            called = true;
            Ok(None)
        },
    )
    .unwrap()
    .unwrap_or_default();
    assert!(called);
    assert!(response.is_empty());
    drop(guard);
    drop(permit);
    assert!(authority.agent_contexts.try_admit(&input, PROFILE).is_ok());
}

#[test]
fn unsupported_modes_legacy_contexts_and_nonsetter_tools_stay_isolated() {
    let files = Files::new();
    let state = files.state(false);
    let authority = ConnAuthority::new(&state.lock().unwrap());
    let hub = SseHub::default();
    let input = line(0, 1);
    let admission = authority.agent_contexts.try_admit(&input, PROFILE).unwrap();
    for mode in [ServeMode::Local, ServeMode::Online] {
        let mut context = ctx(&state, &authority, &hub);
        context.mode = mode;
        assert!(dispatch(
            &context,
            &input,
            PROFILE,
            Some(&files.path()),
            &files.path(),
            &admission,
            true
        )
        .is_none());
    }
    assert!(!candidate(
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":"update_node","arguments":{"node_id":"n0","x":"1"}}})
        .to_string(),
        PROFILE
    ));
    assert!(!candidate(
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":"cut_selected","arguments":{"agentId":"a"}}})
        .to_string(),
        PROFILE
    ));
    assert!(!audited(&C::Undo));
    assert!(!audited(&C::CutSelected));
}

fn property_lines() -> Vec<String> {
    [
        ("set_node_hidden", json!({"value":true})),
        ("set_node_flip", json!({"flip_x":true})),
        ("add_node_effect", json!({"kind":"shadow"})),
        ("set_node_stroke_side_width", json!({"side":"left","width":3})),
    ].into_iter().enumerate().map(|(index,(name,mut arguments))| {
        arguments["node_id"]=json!(format!("n{index}"));
        arguments["agentId"]=json!(format!("property{index}"));
        json!({"jsonrpc":"2.0","id":index,"method":"tools/call","params":{"name":name,"arguments":arguments}}).to_string()
    }).collect()
}

#[test]
fn audited_visibility_flip_effect_and_side_stroke_share_one_durable_save() {
    let files = Files::new();
    let state = files.state(false);
    let authority = ConnAuthority::new(&state.lock().unwrap());
    let hub = SseHub::default();
    let original = serde_json::to_vec(&state.lock().unwrap().editor.doc).unwrap();
    let replies = queued(&ctx(&state, &authority, &hub), &property_lines());
    assert!(replies
        .iter()
        .all(|reply| reply["result"]["isError"] != true && reply.get("error").is_none()));
    assert_eq!(authority.managed_groups.saves.load(Ordering::Acquire), 1);
    let mut state = state.lock().unwrap();
    assert_eq!(state.editor.history.past.len(), 4);
    for _ in 0..4 {
        assert!(state.editor.undo());
    }
    assert_eq!(serde_json::to_vec(&state.editor.doc).unwrap(), original);
}

#[test]
fn extended_property_group_failed_save_restores_every_target_and_undo_step() {
    let files = Files::new();
    let state = files.state(true);
    let authority = ConnAuthority::new(&state.lock().unwrap());
    let hub = SseHub::default();
    let original = serde_json::to_vec(&state.lock().unwrap().editor.doc).unwrap();
    let replies = queued(&ctx(&state, &authority, &hub), &property_lines());
    assert!(replies
        .iter()
        .all(|reply| reply["result"]["isError"] == true || reply.get("error").is_some()));
    let state = state.lock().unwrap();
    assert_eq!(serde_json::to_vec(&state.editor.doc).unwrap(), original);
    assert_eq!(state.editor.history.past.len(), 0);
    assert!(!state.recovery.required());
}

#[test]
fn imported_document_backed_component_catalog_does_not_disable_grouping() {
    let files = Files::new();
    let state = files.state(false);
    {
        let mut state = state.lock().unwrap();
        let roots = &mut state.editor.doc.pages.as_mut().unwrap()[0].children;
        for i in 0..999 {
            roots.push(serde_json::from_value(json!({"type":"frame","id":format!("component{i}"),"reusable":true,"width":10,"height":10,"children":[]})).unwrap());
        }
        let components =
            op_editor_core::components::ComponentLibrary::from_document(&state.editor.doc);
        state.editor.components = components;
        assert_eq!(state.editor.components.len(), 999);
        assert!(state
            .editor
            .components
            .components
            .iter()
            .all(|component| state.editor.components.is_document_backed(&component.id)));
        crate::doc_io::save_to_path_compact(&state.editor, &files.path()).unwrap();
    }
    let authority = ConnAuthority::new(&state.lock().unwrap());
    let hub = SseHub::default();
    let replies = queued(
        &ctx(&state, &authority, &hub),
        &(0..4).map(|i| line(i, 31 + i as i32)).collect::<Vec<_>>(),
    );
    assert!(replies
        .iter()
        .all(|reply| reply.get("error").is_none() && reply["result"]["isError"] != true));
    let metrics = authority.managed_groups.metrics.snapshot();
    assert_eq!(metrics["durableSaves"], 1);
    assert_eq!(metrics["committedChanges"], 4);
    assert_eq!(metrics["savedBatchSizes"]["4"], 1);
    assert_eq!(metrics["budgetRefusals"]["legacy-components"], 0);
}

#[test]
fn eligible_primary_writers_enqueue_while_the_document_mutex_is_busy() {
    let files = Files::new();
    let state = files.state(false);
    let authority = ConnAuthority::new(&state.lock().unwrap());
    let hub = SseHub::default();
    let ctx = ctx(&state, &authority, &hub);
    let held = state.lock().unwrap();
    let (admitted, replies) = std::thread::scope(|scope| {
        let handles = (0..4)
            .map(|i| {
                let ctx = &ctx;
                scope.spawn(move || {
                    let line = line(i, 51 + i as i32);
                    let admission = ctx
                        .authority
                        .agent_contexts
                        .try_admit(&line, PROFILE)
                        .unwrap();
                    dispatch_early(ctx, &line, PROFILE, &admission)
                        .expect("eligible primary route")
                        .unwrap()
                })
            })
            .collect::<Vec<_>>();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while authority
            .managed_groups
            .metrics
            .admitted
            .load(Ordering::Relaxed)
            < 4
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let admitted = authority
            .managed_groups
            .metrics
            .admitted
            .load(Ordering::Relaxed);
        drop(held);
        (
            admitted,
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>(),
        )
    });
    assert_eq!(
        admitted, 4,
        "queue admission must precede the whole-file save lock wait"
    );
    assert_eq!(replies.len(), 4);
    assert_eq!(
        authority.managed_groups.metrics.snapshot()["committedChanges"],
        4
    );
}

#[test]
fn cached_primary_binding_is_invalidated_by_replacement_and_uncertainty() {
    let files = Files::new();
    let state = files.state(false);
    let authority = ConnAuthority::new(&state.lock().unwrap());
    assert!(authority.bound_route().is_some());
    let mut state = state.lock().unwrap();
    state.recovery.replaced();
    assert!(authority.bound_route().is_none());
    authority.refresh_route(&state);
    assert!(authority.bound_route().is_some());
    state.recovery.trip();
    assert!(authority.bound_route().is_none());
    assert!(state.reload_authoritative_file().is_ok());
    assert!(
        authority.bound_route().is_none(),
        "old queued generation must not survive recovery"
    );
    authority.refresh_route(&state);
    assert!(authority.bound_route().is_some());
}

#[path = "managed_group_recovery_tests.rs"]
mod recovery_controls;

#[path = "managed_group_structural_tests.rs"]
mod structural_controls;
