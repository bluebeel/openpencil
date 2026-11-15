//! One complete JSON call per durable admission, without allocating a JSON tree.
use super::tool_profile::McpAccessProfile;
#[path = "cohort_json.rs"]
mod json;

pub(crate) fn parse(
    line: &str,
    profile: McpAccessProfile,
    max_body: usize,
) -> Option<op_mcp::ToolCall> {
    if line.len() > max_body || !json::object(line) {
        return None;
    }
    let call = op_mcp::parse_tool_call(line)?;
    if !call.arguments.contains_key("agentId")
        || profile.refuse_call(&call.tool, &call.arguments).is_some()
        || !super::cohort_coverage::call(&call)
    {
        return None;
    }
    Some(call)
}
