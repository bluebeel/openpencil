//! Opt-in HTTP transport handoff for the ordinary export_nodes MCP result.
//! Only an authenticated tool invocation can register a plan. Its scene is
//! immutable/admitted; all rendering and socket IO happen after dispatch and
//! primary/resident writer scopes have returned.
use super::{raster_snapshot::RasterSnapshot, McpServeError};
use crate::export::RasterFormat;
use op_mcp::{ToolErrorCode, ToolOutcome};
use std::{
    cell::RefCell,
    fmt,
    io::Write,
    marker::PhantomData,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

#[path = "export_stream_body.rs"]
mod body;
#[path = "export_stream_http.rs"]
mod http;

#[derive(Clone, Copy)]
enum Format {
    Raster(RasterFormat),
    Svg,
    Pdf,
}
struct Plan {
    scene: RasterSnapshot,
    ids: Vec<String>,
    format: Format,
    scale: f32,
}
struct Pending {
    marker: String,
    plan: Plan,
}
struct Frame {
    id: u64,
    enabled: bool,
    pending: Option<Pending>,
}
thread_local! { static FRAMES: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) }; }
static NEXT: AtomicU64 = AtomicU64::new(1);

pub(crate) struct Scope {
    id: u64,
    _thread: PhantomData<Rc<()>>,
}
impl Scope {
    pub(crate) fn new(line: &str) -> Self {
        // Ordinary reads/writes do not pay another JSON parse. The marker is
        // never a wire protocol: it exists only inside this transport scope.
        let enabled = line.contains("\"delivery\"")
            && line.contains("\"stream\"")
            && serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .is_some_and(|request| {
                    request["method"] == "tools/call"
                        && request["params"]["name"] == "export_nodes"
                        && request["params"]["arguments"]["delivery"] == "stream"
                });
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        FRAMES.with(|frames| {
            frames.borrow_mut().push(Frame {
                id,
                enabled,
                pending: None,
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
        drop(frame); // scene and admission leave on normal return/error/panic
    }
}

#[derive(Debug)]
enum PrepareError {
    UnsupportedTransport,
    AlreadyPrepared,
    CombinedPdf,
    Format(String),
    PdfMode(String),
}
impl fmt::Display for PrepareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedTransport => f.write_str("delivery=stream requires a direct export_nodes tools/call over headless HTTP; use inline on other transports"),
            Self::AlreadyPrepared => f.write_str("only one export stream may be prepared per HTTP response"),
            Self::CombinedPdf => f.write_str("stream delivery currently requires pdfMode=separate; use inline for combined PDF"),
            Self::Format(format) => write!(f, "unknown format {format:?}: must be one of png, jpeg, webp, pdf, svg"),
            Self::PdfMode(mode) => write!(f, "pdfMode must be separate or combined, got {mode:?}"),
        }
    }
}
impl std::error::Error for PrepareError {}

pub(super) fn prepare(
    scene: RasterSnapshot,
    ids: Vec<String>,
    format: &str,
    scale: f32,
    pdf_mode: &str,
) -> ToolOutcome {
    let result = (|| {
        let format = match format {
            "png" => Format::Raster(RasterFormat::Png),
            "jpeg" | "jpg" => Format::Raster(RasterFormat::Jpeg),
            "webp" => Format::Raster(RasterFormat::Webp),
            "svg" => Format::Svg,
            "pdf" => {
                match pdf_mode {
                    "separate" => {}
                    "combined" => return Err(PrepareError::CombinedPdf),
                    other => return Err(PrepareError::PdfMode(other.into())),
                }
                Format::Pdf
            }
            other => return Err(PrepareError::Format(other.into())),
        };
        if matches!(format, Format::Svg) && scene.active_page().is_none() {
            return Ok(ToolOutcome::Err(
                ToolErrorCode::ToolFailed,
                "no active page".into(),
            ));
        }
        FRAMES.with(|frames| {
            let mut frames = frames.borrow_mut();
            let frame = frames
                .last_mut()
                .filter(|frame| frame.enabled)
                .ok_or(PrepareError::UnsupportedTransport)?;
            if frame.pending.is_some() {
                return Err(PrepareError::AlreadyPrepared);
            }
            let marker = format!("{{\"__op_export_stream_pending\":{}}}", frame.id);
            frame.pending = Some(Pending {
                marker: marker.clone(),
                plan: Plan {
                    scene,
                    ids,
                    format,
                    scale,
                },
            });
            Ok(ToolOutcome::OkJson(marker))
        })
    })();
    result
        .unwrap_or_else(|error| ToolOutcome::Err(ToolErrorCode::InvalidArgument, error.to_string()))
}

/// Existing profile/file routing and agent-context postprocessing construct
/// their normal (small) response first. Preserve its outer envelope data,
/// including request ID and any result._meta, replacing only the text value.
/// A denied/failed tool has no matching placeholder and follows normal IO.
pub(super) fn try_write<S: Write>(
    stream: &mut S,
    status: &str,
    response: &str,
    origin: Option<&str>,
) -> Option<Result<(), McpServeError>> {
    if status != "200 OK" {
        return None;
    }
    let marker = FRAMES.with(|frames| {
        frames
            .borrow()
            .last()
            .and_then(|frame| frame.pending.as_ref())
            .map(|pending| pending.marker.clone())
    })?;
    let envelope = serde_json::from_str::<serde_json::Value>(response).ok()?;
    if envelope["result"]["isError"] == true
        || envelope["result"]["content"][0]["text"].as_str() != Some(marker.as_str())
    {
        return None;
    }
    let pending = FRAMES.with(|frames| {
        frames
            .borrow_mut()
            .last_mut()
            .and_then(|frame| frame.pending.take())
    })?;
    Some(
        (|| {
            let mut chunks = http::Chunks::start(stream, origin)?;
            let mut plan = Some(pending.plan);
            write_envelope(
                &envelope,
                &["result", "content", "0", "text"],
                &mut chunks,
                &mut plan,
            )?;
            chunks.finish()
        })()
        .map_err(|error: std::io::Error| McpServeError::Io(format!("export stream: {error}"))),
    )
}

// Select the text value structurally, not by replacing a marker substring:
// arbitrary request IDs/metadata may legitimately equal the same string.
fn write_envelope<W: Write>(
    value: &serde_json::Value,
    path: &[&str],
    output: &mut W,
    plan: &mut Option<Plan>,
) -> std::io::Result<()> {
    if path.is_empty() {
        let plan = plan
            .take()
            .ok_or_else(|| std::io::Error::other("export stream plan consumed twice"))?;
        output.write_all(b"\"")?;
        body::render(plan, output)?;
        return output.write_all(b"\"");
    }
    match value {
        serde_json::Value::Object(fields) => {
            output.write_all(b"{")?;
            for (index, (key, value)) in fields.iter().enumerate() {
                if index != 0 {
                    output.write_all(b",")?;
                }
                serde_json::to_writer(&mut *output, key).map_err(std::io::Error::other)?;
                output.write_all(b":")?;
                if key == path[0] {
                    write_envelope(value, &path[1..], output, plan)?;
                } else {
                    serde_json::to_writer(&mut *output, value).map_err(std::io::Error::other)?;
                }
            }
            output.write_all(b"}")
        }
        serde_json::Value::Array(values) if path[0] == "0" => {
            output.write_all(b"[")?;
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.write_all(b",")?;
                }
                if index == 0 {
                    write_envelope(value, &path[1..], output, plan)?;
                } else {
                    serde_json::to_writer(&mut *output, value).map_err(std::io::Error::other)?;
                }
            }
            output.write_all(b"]")
        }
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "export stream response path changed",
        )),
    }
}
