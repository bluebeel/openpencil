//! Move prepared replies out of registries constructed for one request.
use super::*;
impl ToolRegistry {
    /// Consume this request-local registry. Public reusable registry dispatch
    /// remains unchanged; no shared tool state is cleared or cached by revision.
    pub fn dispatch_once(mut self, call: ToolCall) -> ToolResponse {
        let Some(tool) = self.tools.remove(&call.tool) else {
            return ToolResponse::Err {
                id: call.id,
                code: ToolErrorCode::UnknownTool,
                message: format!("unknown tool: {}", call.tool),
            };
        };
        into_response(call.id, tool.call_once(&call.arguments))
    }
}
pub(super) fn into_response(id: RequestId, outcome: ToolOutcome) -> ToolResponse {
    match outcome {
        ToolOutcome::Ok(result) => ToolResponse::Ok {
            id: id,
            result,
            command: None,
            json: None,
            image: None,
        },
        ToolOutcome::OkWithCommand(result, command) => ToolResponse::Ok {
            id: id,
            result,
            command: Some(command),
            json: None,
            image: None,
        },
        ToolOutcome::OkJson(json) => ToolResponse::Ok {
            id: id,
            result: BTreeMap::new(),
            command: None,
            json: Some(json),
            image: None,
        },
        ToolOutcome::OkJsonWithCommand(json, command) => ToolResponse::Ok {
            id: id,
            result: BTreeMap::new(),
            command: Some(command),
            json: Some(json),
            image: None,
        },
        ToolOutcome::OkImageContent {
            image_base64,
            mime_type,
            metadata_json,
        } => ToolResponse::Ok {
            id: id,
            result: BTreeMap::new(),
            command: None,
            json: metadata_json,
            image: Some(Box::new(ImageContent {
                data: image_base64,
                mime_type,
            })),
        },
        ToolOutcome::Err(code, message) => ToolResponse::Err {
            id: id,
            code,
            message,
        },
    }
}
