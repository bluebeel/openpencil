//! Defer trusted HTTP export calls past the document authority boundary.
//! Public/custom registries outside these transport scopes stay synchronous.
use super::McpServeError;
use crate::export::WorkCancellation;
use op_mcp::{McpTool, ToolCall, ToolErrorCode, ToolOutcome, ToolRegistry, ToolResponse};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    marker::PhantomData,
    rc::Rc,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

fn eligible(name: &str) -> bool {
    matches!(
        name,
        "export_nodes" | "export_item" | "get_screenshot" | "export_frames"
    )
}
struct Pending {
    tool: Arc<dyn McpTool>,
    call: ToolCall,
}
struct Frame {
    id: u64,
    call: Option<ToolCall>,
    pending: Option<Pending>,
    executing: bool,
    cancel: WorkCancellation,
}
thread_local! { static FRAMES: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) }; }
static NEXT: AtomicU64 = AtomicU64::new(1);
pub(crate) struct Scope {
    id: u64,
    _thread: PhantomData<Rc<()>>,
}
impl Scope {
    pub(crate) fn new(line: &str) -> Self {
        let call = [
            "export_nodes",
            "export_item",
            "get_screenshot",
            "export_frames",
        ]
        .iter()
        .any(|name| line.contains(name))
        .then(|| op_mcp::parse_tool_call(line))
        .flatten()
        .filter(|call| eligible(&call.tool))
        .filter(|_| super::sniff_method(line).as_deref() == Some("tools/call"));
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        FRAMES.with(|frames| {
            frames.borrow_mut().push(Frame {
                id,
                call,
                pending: None,
                executing: false,
                cancel: WorkCancellation::new(),
            })
        });
        Self {
            id,
            _thread: PhantomData,
        }
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        let frame = FRAMES.with(|frames| {
            let mut frames = frames.borrow_mut();
            frames
                .iter()
                .position(|frame| frame.id == self.id)
                .map(|index| frames.remove(index))
        });
        if let Some(frame) = frame {
            frame.cancel.cancel();
            drop(frame);
        }
    }
}
pub(super) fn capture_token() -> Option<WorkCancellation> {
    FRAMES.with(|frames| {
        frames
            .borrow()
            .last()
            .filter(|frame| frame.call.is_some() && !frame.executing)
            .map(|frame| frame.cancel.clone())
    })
}
pub(super) fn executing() -> bool {
    FRAMES.with(|frames| frames.borrow().last().is_some_and(|frame| frame.executing))
}
pub(super) fn cancel_current() {
    FRAMES.with(|frames| {
        if let Some(frame) = frames.borrow().last() {
            frame.cancel.cancel();
        }
    });
}
struct Deferred(Arc<dyn McpTool>);
impl McpTool for Deferred {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn call(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        FRAMES.with(|frames| {
            let mut frames = frames.borrow_mut();
            let Some(frame) = frames.last_mut().filter(|frame| !frame.executing) else {
                return ToolOutcome::Err(
                    ToolErrorCode::Internal,
                    "HTTP export scope is unavailable".into(),
                );
            };
            let Some(mut call) = frame.call.clone().filter(|call| call.tool == self.name()) else {
                return ToolOutcome::Err(
                    ToolErrorCode::Internal,
                    "HTTP export scope identity changed".into(),
                );
            };
            if frame.pending.is_some() {
                return ToolOutcome::Err(
                    ToolErrorCode::Internal,
                    "HTTP export already prepared".into(),
                );
            }
            call.arguments = args.clone();
            frame.pending = Some(Pending {
                tool: Arc::clone(&self.0),
                call,
            });
            ToolOutcome::OkJson(marker(frame.id))
        })
    }
}
pub(super) fn wrap(tool: Box<dyn McpTool>) -> Box<dyn McpTool> {
    if eligible(tool.name()) && capture_token().is_some() {
        Box::new(Deferred(Arc::from(tool)))
    } else {
        tool
    }
}
fn marker(id: u64) -> String {
    format!("{{\"__op_http_export_pending\":{id}}}")
}
struct Executing(u64);
impl Drop for Executing {
    fn drop(&mut self) {
        FRAMES.with(|frames| {
            if let Some(frame) = frames
                .borrow_mut()
                .iter_mut()
                .find(|frame| frame.id == self.0)
            {
                frame.executing = false;
            }
        });
    }
}
struct OwnedTool(Arc<dyn McpTool>);
impl McpTool for OwnedTool {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn call(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        self.0.call(args)
    }
}

/// Called by the HTTP writer only, after primary/resident dispatch returned.
/// Invoke the unchanged tool through the ordinary registry serializer so image
/// blocks, errors, IDs and default inline data retain their established shape.
pub(super) fn complete_response(response: &str) -> Result<Option<String>, McpServeError> {
    let id = FRAMES.with(|frames| {
        frames
            .borrow()
            .last()
            .filter(|frame| frame.pending.is_some())
            .map(|frame| frame.id)
    });
    let Some(id) = id else {
        return Ok(None);
    };
    let envelope = serde_json::from_str::<serde_json::Value>(response)
        .map_err(|error| McpServeError::Protocol(format!("deferred export envelope: {error}")))?;
    if envelope["result"]["content"][0]["text"].as_str() != Some(marker(id).as_str())
        || envelope["result"]["isError"] == true
    {
        return Ok(None);
    }
    let pending = FRAMES.with(|frames| {
        let mut frames = frames.borrow_mut();
        let frame = frames.last_mut().expect("scope checked above");
        frame.executing = true;
        frame.pending.take().expect("pending checked above")
    });
    let _executing = Executing(id);
    let mut registry = ToolRegistry::default();
    registry.register(Box::new(OwnedTool(pending.tool)));
    let result = registry.dispatch(pending.call);
    if matches!(
        &result,
        ToolResponse::Ok {
            command: Some(_),
            ..
        }
    ) {
        return Err(McpServeError::Dispatch(
            "a deferred read-export unexpectedly requested a document command".into(),
        ));
    }
    let mut actual = op_mcp::tool_response_to_json(&result);
    if let Some(meta) = envelope["result"].get("_meta") {
        // The ordinary serializer has no outer result._meta. Append only
        // this small post-context metadata; never parse/copy a full inline
        // base64 response merely to carry the retention warning forward.
        if !actual.ends_with("}}") {
            return Err(McpServeError::Protocol(
                "deferred export response envelope changed".into(),
            ));
        }
        actual.truncate(actual.len() - 2);
        actual.push_str(",\"_meta\":");
        actual.push_str(&meta.to_string());
        actual.push_str("}}");
    }
    Ok(Some(actual))
}
