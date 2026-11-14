//! Single parsed tool calls consume their request-local prepared registry.
use super::*;
pub(super) fn process(
    registry: ToolRegistry,
    call: op_mcp::ToolCall,
    mut apply: impl FnMut(&str, &EditorCommand) -> bool,
) -> String {
    let tool_name = call.tool.clone();
    let mut response = registry.dispatch_once(call);
    if let op_mcp::ToolResponse::Ok {
        id,
        command: Some(command),
        ..
    } = &response
    {
        if !apply(&tool_name, command) {
            response = op_mcp::ToolResponse::Err {
                id: id.clone(),
                code: op_mcp::ToolErrorCode::Internal,
                message: format!("host rejected command: {command:?}"),
            };
        }
    }
    op_mcp::tool_response_to_json(&response)
}
