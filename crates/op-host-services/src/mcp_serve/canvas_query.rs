//! Read-only headless picking/marquee queries using the editor's actual scene.
//! Batch queries share one layout snapshot and retain only their small results.
use op_editor_ui::snapping::{SnapAxis, SnapIndex, SnapRect};
use std::{
    collections::{BTreeMap, HashMap},
    fmt,
};
#[path = "canvas_snap.rs"]
mod canvas_snap;

use op_editor_core::{
    render_backend::{Point2D, Rect},
    EditorState,
};
use op_mcp::{McpTool, ToolErrorCode, ToolOutcome};
use serde::Deserialize;
use serde_json::{json, Value};

const MAX_QUERIES: usize = 4096;
const MAX_QUERY_BYTES: usize = 1024 * 1024;

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Query {
    Point {
        x: f32,
        y: f32,
        #[serde(default = "default_zoom")]
        zoom: f32,
    },
    Snap {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        #[serde(default)]
        dx: f32,
        #[serde(default)]
        dy: f32,
        #[serde(default = "default_zoom")]
        zoom: f32,
        #[serde(default = "default_threshold")]
        threshold: f32,
        #[serde(default, rename = "parentId")]
        parent: Option<String>,
        #[serde(default, rename = "excludeIds")]
        excluded: Vec<String>,
    },
    Rect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
}

fn default_zoom() -> f32 {
    1.0
}
fn default_threshold() -> f32 {
    6.0
}

#[derive(Debug)]
enum QueryError {
    InvalidField(&'static str),
    InvalidBatch(String),
    BatchLimit,
    NoActivePage,
    MissingParent(String),
}
impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidField(name) => {
                write!(f, "{name} must be a finite number; zoom must be positive")
            }
            Self::InvalidBatch(reason) => write!(f, "invalid queries_json: {reason}"),
            Self::BatchLimit => write!(
                f,
                "provide 1..{MAX_QUERIES} queries within {MAX_QUERY_BYTES} bytes"
            ),
            Self::NoActivePage => f.write_str("the document has no active page"),
            Self::MissingParent(id) => write!(f, "snap parent {id} is not on the active page"),
        }
    }
}
impl std::error::Error for QueryError {}

fn queries(args: &BTreeMap<String, String>) -> Result<Vec<Query>, QueryError> {
    fn number(args: &BTreeMap<String, String>, key: &'static str) -> Result<f32, QueryError> {
        args.get(key)
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .ok_or(QueryError::InvalidField(key))
    }
    let queries = if let Some(batch) = args.get("queries_json") {
        if batch.len() > MAX_QUERY_BYTES {
            return Err(QueryError::BatchLimit);
        }
        serde_json::from_str::<Vec<Query>>(batch)
            .map_err(|e| QueryError::InvalidBatch(e.to_string()))?
    } else {
        let x = number(args, "x")?;
        let y = number(args, "y")?;
        vec![
            match args.get("kind").map(String::as_str).unwrap_or("point") {
                "point" => Query::Point {
                    x,
                    y,
                    zoom: if args.contains_key("zoom") {
                        number(args, "zoom")?
                    } else {
                        1.0
                    },
                },
                "rect" => Query::Rect {
                    x,
                    y,
                    width: number(args, "width")?,
                    height: number(args, "height")?,
                },
                "snap" => Query::Snap {
                    x,
                    y,
                    width: number(args, "width")?,
                    height: number(args, "height")?,
                    dx: if args.contains_key("dx") {
                        number(args, "dx")?
                    } else {
                        0.0
                    },
                    dy: if args.contains_key("dy") {
                        number(args, "dy")?
                    } else {
                        0.0
                    },
                    zoom: if args.contains_key("zoom") {
                        number(args, "zoom")?
                    } else {
                        1.0
                    },
                    threshold: if args.contains_key("threshold") {
                        number(args, "threshold")?
                    } else {
                        6.0
                    },
                    parent: args.get("parentId").cloned(),
                    excluded: args
                        .get("exclude_ids_json")
                        .map(|v| serde_json::from_str(v))
                        .transpose()
                        .map_err(|e| QueryError::InvalidBatch(e.to_string()))?
                        .unwrap_or_default(),
                },
                _ => return Err(QueryError::InvalidField("kind (point, rect or snap)")),
            },
        ]
    };
    if queries.is_empty() || queries.len() > MAX_QUERIES {
        return Err(QueryError::BatchLimit);
    }
    for query in &queries {
        let valid = match *query {
            Query::Point { x, y, zoom } => {
                x.is_finite() && y.is_finite() && zoom.is_finite() && zoom > 0.0
            }
            Query::Rect {
                x,
                y,
                width,
                height,
            } => [x, y, width, height, x + width, y + height]
                .iter()
                .all(|v| v.is_finite()),
            Query::Snap {
                x,
                y,
                width,
                height,
                dx,
                dy,
                zoom,
                threshold,
                ..
            } => {
                [
                    x,
                    y,
                    width,
                    height,
                    dx,
                    dy,
                    zoom,
                    threshold,
                    x + width,
                    y + height,
                    x + dx,
                    y + dy,
                ]
                .iter()
                .all(|v| v.is_finite())
                    && zoom > 0.0
                    && threshold >= 0.0
            }
        };
        if !valid {
            return Err(QueryError::InvalidField("query coordinates"));
        }
    }
    Ok(queries)
}

pub(super) struct QueryCanvas {
    result: Result<Value, QueryError>,
}

impl McpTool for QueryCanvas {
    fn name(&self) -> &str {
        "query_canvas"
    }
    fn call(&self, _: &BTreeMap<String, String>) -> ToolOutcome {
        match &self.result {
            Ok(result) => ToolOutcome::OkJson(result.to_string()),
            Err(error) => ToolOutcome::Err(
                if matches!(error, QueryError::NoActivePage) {
                    ToolErrorCode::ToolFailed
                } else {
                    ToolErrorCode::InvalidArgument
                },
                error.to_string(),
            ),
        }
    }
}

pub(super) fn snapshot(
    state: &EditorState,
    arguments: Option<&BTreeMap<String, String>>,
) -> QueryCanvas {
    let result = (|| {
        // Discovery is cheap, and malformed calls never build a scene.
        let queries = queries(arguments.ok_or(QueryError::InvalidField("query"))?)?;
        let scene = op_pen_loader::editor_state_to_active_page_layout_scene(state);
        let page_id = scene
            .active_page()
            .ok_or(QueryError::NoActivePage)?
            .id
            .clone();
        let mut results = Vec::with_capacity(queries.len());
        let mut snap_indexes: HashMap<Option<String>, SnapIndex> = HashMap::new();
        for query in queries {
            results.push(match query {
                Query::Point { x, y, zoom } => {
                    let path = scene.node_path_at_doc_point(Point2D::new(x, y), zoom);
                    json!({"kind":"point", "nodeId":path.as_ref().and_then(|p| p.last()), "path":path})
                }
                Query::Rect { x, y, width, height } => {
                    let ids = scene.nodes_intersecting_doc_rect(Rect {
                        origin: Point2D::new(x, y), size: Point2D::new(width, height),
                    });
                    json!({"kind":"rect", "nodeIds":ids})
                }
                Query::Snap {x,y,width,height,dx,dy,zoom,threshold,parent,excluded} => {
                    if !snap_indexes.contains_key(&parent) {
                        let index=canvas_snap::index(&scene,parent.as_deref())
                            .ok_or_else(||QueryError::MissingParent(parent.clone().unwrap_or_default()))?;
                        snap_indexes.insert(parent.clone(),index);
                    }
                    let result=snap_indexes[&parent].query(SnapRect{x,y,width,height},dx,dy,zoom,threshold,&excluded)
                        .ok_or(QueryError::InvalidField("snap geometry"))?;
                    let guides:Vec<_>=result.guides.iter().map(|guide|json!({
                        "axis":if guide.axis==SnapAxis::X {"x"} else {"y"},"position":guide.position,
                        "start":guide.start,"end":guide.end,"nodeId":guide.target_id,
                    })).collect();
                    json!({"kind":"snap","dx":result.dx,"dy":result.dy,"guides":guides})
                }
            });
        }
        Ok(json!({"pageId":page_id, "coordinateSpace":"document", "results":results}))
    })();
    QueryCanvas { result }
}
