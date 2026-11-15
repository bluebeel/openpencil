//! Single parsed tool calls consume their request-local prepared registry.
use super::*;
#[path = "kit_insert_reply.rs"]
mod kit_insert_reply;
pub(super) use kit_insert_reply::apply as apply_with_receipt;
pub(super) struct AppliedReply {
    pub(super) applied: bool,
    pub(super) kit_root: Option<op_editor_core::NodeId>,
    pub(super) detached: Option<op_editor_core::ComponentDetachResult>,
}
pub(super) fn process(
    registry: ToolRegistry,
    call: op_mcp::ToolCall,
    mut apply: impl FnMut(&str, &EditorCommand) -> AppliedReply,
) -> String {
    let tool_name = call.tool.clone();
    let mut response = registry.dispatch_once(call);
    if let op_mcp::ToolResponse::Ok {
        id,
        command: Some(command),
        ..
    } = &response
    {
        let applied = apply(&tool_name, command);
        if !applied.applied {
            response = op_mcp::ToolResponse::Err {
                id: id.clone(),
                code: if matches!(
                    command,
                    EditorCommand::DetachComponent { .. }
                        | EditorCommand::SetInstanceComponent { .. }
                ) {
                    op_mcp::ToolErrorCode::ToolFailed
                } else {
                    op_mcp::ToolErrorCode::Internal
                },
                message: format!(
                    "host rejected command: {}",
                    op_util::bounded_debug::summary(command, 1024)
                ),
            };
        } else {
            if let Some(root) = applied.kit_root {
                enrich_kit_root(&mut response, &root);
            }
            if let Some(detached) = applied.detached {
                enrich_detach(&mut response, detached);
            }
        }
    }
    op_mcp::tool_response_to_json(&response)
}

fn enrich_kit_root(response: &mut op_mcp::ToolResponse, root: &op_editor_core::NodeId) {
    let op_mcp::ToolResponse::Ok {
        result,
        command:
            Some(EditorCommand::InstantiateKitComponent {
                kit_id,
                component_id,
                ..
            }),
        json,
        image,
        ..
    } = response
    else {
        return;
    };
    // Preserve custom JSON/image schemas. The ordinary element tool owns this
    // descriptor shape; every original field survives in the richer object.
    if json.is_some()
        || image.is_some()
        || result.get("kit_id").map(String::as_str) != Some(kit_id.as_str())
        || result.get("component_id").map(String::as_str) != Some(component_id.as_str())
    {
        return;
    }
    result.insert("node_id".into(), root.as_str().into());
    let mut fields = result
        .iter()
        .map(|(key, value)| (key.clone(), serde_json::Value::String(value.clone())))
        .collect::<serde_json::Map<_, _>>();
    fields.insert("newIds".into(), serde_json::json!([root.as_str()]));
    *json = Some(serde_json::Value::Object(fields).to_string());
}

fn enrich_detach(
    response: &mut op_mcp::ToolResponse,
    detached: op_editor_core::ComponentDetachResult,
) {
    let op_mcp::ToolResponse::Ok {
        result,
        command: Some(EditorCommand::DetachComponent { .. }),
        json,
        image,
        ..
    } = response
    else {
        return;
    };
    if json.is_some() || image.is_some() {
        return;
    }
    result.insert("node_id".into(), detached.node_id.as_str().into());
    result.insert("result_receipt".into(), "verified".into());
    let mut fields = result
        .iter()
        .map(|(key, value)| (key.clone(), serde_json::Value::String(value.clone())))
        .collect::<serde_json::Map<_, _>>();
    fields.insert(
        "created_root".into(),
        serde_json::Value::Bool(detached.created_root),
    );
    fields.insert(
        "newRootIds".into(),
        if detached.created_root {
            serde_json::json!([detached.node_id.as_str()])
        } else {
            serde_json::json!([])
        },
    );
    *json = Some(serde_json::Value::Object(fields).to_string());
}
