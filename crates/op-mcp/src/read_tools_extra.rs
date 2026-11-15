//! Misc query/read MCP tools, split out of `read_tools.rs` to keep that file
//! under the repo 800-line cap: get_canvas_bounds / find_node_by_name /
//! get_node_parent / count_nodes / list_node_kinds / get_history_depth /
//! get_viewport / get_selection_set. These are self-contained snapshot tools
//! (no dependency on the layout/empty-space helpers that remain in
//! `read_tools.rs`).

use super::tools::read_walk;
use std::{collections::BTreeMap, fmt::Write};

use jian_ops_schema::node::PenNode;
use op_editor_core::pen_node_ext::PenNodeExt;
use op_editor_core::EditorState;

use super::tools::{active_children, kind_label};
use super::{McpTool, ToolErrorCode, ToolOutcome};

/// First-party `get_canvas_bounds` tool — union bounding box of every
/// top-level node on the active page.
pub struct GetCanvasBounds {
    pub bounds: Option<(i32, i32, i32, i32)>,
}

impl McpTool for GetCanvasBounds {
    fn name(&self) -> &str {
        "get_canvas_bounds"
    }
    fn call(&self, _args: &BTreeMap<String, String>) -> ToolOutcome {
        let mut out = BTreeMap::new();
        match self.bounds {
            Some((x, y, w, h)) => {
                out.insert("x".into(), x.to_string());
                out.insert("y".into(), y.to_string());
                out.insert("w".into(), w.to_string());
                out.insert("h".into(), h.to_string());
                out.insert("has_content".into(), "true".into());
            }
            None => {
                out.insert("x".into(), "0".into());
                out.insert("y".into(), "0".into());
                out.insert("w".into(), "0".into());
                out.insert("h".into(), "0".into());
                out.insert("has_content".into(), "false".into());
            }
        }
        ToolOutcome::Ok(out)
    }
}

pub fn get_canvas_bounds_snapshot(state: &EditorState) -> GetCanvasBounds {
    canvas_bounds(state, false)
}
/// Trusted-host revision-keyed reuse; the original factory remains fresh.
pub fn get_canvas_bounds_snapshot_cached(state: &EditorState) -> GetCanvasBounds {
    canvas_bounds(state, true)
}
fn canvas_bounds(state: &EditorState, cached: bool) -> GetCanvasBounds {
    let page = match &state.doc.pages {
        Some(pages) if pages.is_empty() => return GetCanvasBounds { bounds: None },
        Some(pages) => state.ui.active_page_index.min(pages.len() - 1),
        None => 0,
    };
    let roots = if cached {
        op_pen_loader::editor_state_to_page_item_geometry_cached(state, page, None)
    } else {
        std::sync::Arc::new(op_pen_loader::editor_state_to_page_item_geometry(
            state, page, None,
        ))
    };
    GetCanvasBounds {
        bounds: jian_scene::layout_scene::LayoutScene::content_bounds_from_rects(
            roots
                .iter()
                .map(op_pen_loader::ComputedGeometry::visual_bounds),
        )
        .map(|bounds| {
            (
                bounds.origin.x as i32,
                bounds.origin.y as i32,
                bounds.size.x as i32,
                bounds.size.y as i32,
            )
        }),
    }
}

// --- find_node_by_name -----------------------------------------------

/// First-party `find_node_by_name` tool — locate the first node whose
/// `name` matches the arg, anywhere on the active page.
pub struct FindNodeByName {
    pub index: Vec<(String, String, String)>,
}

impl McpTool for FindNodeByName {
    fn name(&self) -> &str {
        "find_node_by_name"
    }
    fn call(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        let Some(query) = args.get("name") else {
            return ToolOutcome::Err(ToolErrorCode::MissingArgument, "name is required".into());
        };
        let Some((_, id, kind)) = self.index.iter().find(|(n, _, _)| n == query) else {
            return ToolOutcome::Err(
                ToolErrorCode::ToolFailed,
                format!("no node named {query:?} on the active page"),
            );
        };
        let mut out = BTreeMap::new();
        out.insert("id".into(), id.to_string());
        out.insert("kind".into(), kind.clone());
        ToolOutcome::Ok(out)
    }
}

pub fn find_node_by_name_snapshot(state: &EditorState) -> FindNodeByName {
    fn walk(nodes: &[PenNode], out: &mut Vec<(String, String, String)>) {
        for entry in read_walk::nodes(nodes) {
            let node = entry.node;
            out.push((
                node.base().name.clone().unwrap_or_default(),
                node.id_str().to_string(),
                kind_label(node).to_string(),
            ));
        }
    }
    let mut index = Vec::new();
    walk(active_children(state), &mut index);
    FindNodeByName { index }
}

/// Request-specific exact-name lookup on the active page; first DFS match wins.
pub fn find_node_by_name_snapshot_for_call(
    state: &EditorState,
    args: &BTreeMap<String, String>,
) -> FindNodeByName {
    let index = args
        .get("name")
        .and_then(|name| {
            first_match(active_children(state), "", 0, &|node| {
                node.base().name.as_deref().unwrap_or_default() == name
            })
        })
        .map(|(node, _, _)| {
            vec![(
                node.base().name.clone().unwrap_or_default(),
                node.id_str().into(),
                kind_label(node).into(),
            )]
        })
        .unwrap_or_default();
    FindNodeByName { index }
}

fn first_match<'a>(
    nodes: &'a [PenNode],
    parent: &'a str,
    depth: u32,
    matches: &impl Fn(&PenNode) -> bool,
) -> Option<(&'a PenNode, &'a str, u32)> {
    read_walk::nodes(nodes)
        .find(|entry| matches(entry.node))
        .map(|entry| {
            (
                entry.node,
                if entry.depth == 0 {
                    parent
                } else {
                    entry.parent
                },
                depth + entry.depth as u32,
            )
        })
}

// --- get_node_parent -------------------------------------------------

/// First-party `get_node_parent` tool — parent id of `node_id` on the
/// active page.
pub struct GetNodeParent {
    pub index: Vec<(String, String, u32)>,
}

impl McpTool for GetNodeParent {
    fn name(&self) -> &str {
        "get_node_parent"
    }
    fn call(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        let Some(raw) = args.get("node_id") else {
            return ToolOutcome::Err(ToolErrorCode::MissingArgument, "node_id is required".into());
        };
        let node_id: &str = raw.as_str();
        let Some((_, parent_id, depth)) = self.index.iter().find(|(id, _, _)| id == node_id) else {
            return ToolOutcome::Err(
                ToolErrorCode::ToolFailed,
                format!("node {node_id} not found on active page"),
            );
        };
        let mut out = BTreeMap::new();
        out.insert("parent_id".into(), parent_id.to_string());
        out.insert("depth".into(), depth.to_string());
        ToolOutcome::Ok(out)
    }
}

pub fn get_node_parent_snapshot(state: &EditorState) -> GetNodeParent {
    fn walk(nodes: &[PenNode], parent: &str, depth: u32, out: &mut Vec<(String, String, u32)>) {
        for entry in read_walk::nodes(nodes) {
            out.push((
                entry.node.id_str().to_string(),
                if entry.depth == 0 {
                    parent
                } else {
                    entry.parent
                }
                .to_string(),
                depth + entry.depth as u32,
            ));
        }
    }
    let mut index = Vec::new();
    walk(active_children(state), "", 0, &mut index);
    GetNodeParent { index }
}

/// Parent queries intentionally use the active page and the first ID match,
/// unlike get_node's global last-match contract.
pub fn get_node_parent_snapshot_for_call(
    state: &EditorState,
    args: &BTreeMap<String, String>,
) -> GetNodeParent {
    let Some(id) = args.get("node_id") else {
        return GetNodeParent { index: Vec::new() };
    };
    if let Some(lookup) = state
        .trusted_node_lookup(active_children(state))
        .filter(|lookup| lookup.is_indexed())
    {
        let index = lookup
            .find(id, op_editor_core::NodeReadOrder::First)
            .map(|found| {
                vec![(
                    found.node.id_str().into(),
                    found.parent.map_or("", PenNodeExt::id_str).into(),
                    found.depth as u32,
                )]
            })
            .unwrap_or_default();
        return GetNodeParent { index };
    }
    let index = args
        .get("node_id")
        .and_then(|id| first_match(active_children(state), "", 0, &|node| node.id_str() == id))
        .map(|(node, parent, depth)| vec![(node.id_str().into(), parent.into(), depth)])
        .unwrap_or_default();
    GetNodeParent { index }
}

// --- count_nodes -----------------------------------------------------

/// First-party `count_nodes` tool — total node count + per-page
/// breakdown.
pub struct CountNodes {
    pub per_page: Vec<u32>,
}

impl McpTool for CountNodes {
    fn name(&self) -> &str {
        "count_nodes"
    }
    fn call(&self, _args: &BTreeMap<String, String>) -> ToolOutcome {
        let total: u32 = self.per_page.iter().sum();
        let mut encoded = String::new();
        for (index, count) in self.per_page.iter().enumerate() {
            if index > 0 {
                encoded.push(';');
            }
            let _ = write!(encoded, "{index}|{count}");
        }
        let mut out = BTreeMap::new();
        out.insert("total".into(), total.to_string());
        out.insert("per_page".into(), encoded);
        ToolOutcome::Ok(out)
    }
}

pub fn count_nodes_snapshot(state: &EditorState) -> CountNodes {
    fn walk(nodes: &[PenNode]) -> u32 {
        read_walk::nodes(nodes).count() as u32
    }
    let per_page = match state.doc.pages.as_ref() {
        Some(pages) => pages.iter().map(|p| walk(&p.children)).collect(),
        None => vec![walk(&state.doc.children)],
    };
    CountNodes { per_page }
}

// --- list_node_kinds -------------------------------------------------

/// First-party `list_node_kinds` tool — per-kind histogram of nodes on
/// the active page.
pub struct ListNodeKinds {
    pub histogram: Vec<(String, u32)>,
}

impl McpTool for ListNodeKinds {
    fn name(&self) -> &str {
        "list_node_kinds"
    }
    fn call(&self, _args: &BTreeMap<String, String>) -> ToolOutcome {
        let mut encoded = String::new();
        for (index, (kind, count)) in self.histogram.iter().enumerate() {
            if index > 0 {
                encoded.push(';');
            }
            let _ = write!(encoded, "{kind}|{count}");
        }
        let mut out = BTreeMap::new();
        out.insert("distinct".into(), self.histogram.len().to_string());
        out.insert("kinds".into(), encoded);
        ToolOutcome::Ok(out)
    }
}

pub fn list_node_kinds_snapshot(state: &EditorState) -> ListNodeKinds {
    fn walk(nodes: &[PenNode], counts: &mut BTreeMap<&'static str, u32>) {
        for entry in read_walk::nodes(nodes) {
            *counts.entry(kind_label(entry.node)).or_insert(0) += 1;
        }
    }
    let mut counts: BTreeMap<&'static str, u32> = BTreeMap::new();
    walk(active_children(state), &mut counts);
    let histogram = counts
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    ListNodeKinds { histogram }
}

// --- get_history_depth -----------------------------------------------

/// First-party `get_history_depth` tool — undo + redo stack sizes.
pub struct GetHistoryDepth {
    pub past: usize,
    pub future: usize,
    pub actor_past: usize,
    pub actor_future: usize,
    pub undo_blocked: bool,
    pub redo_blocked: bool,
    pub retention: op_editor_core::HistoryRetentionStatus,
}

impl McpTool for GetHistoryDepth {
    fn name(&self) -> &str {
        "get_history_depth"
    }
    fn call(&self, _args: &BTreeMap<String, String>) -> ToolOutcome {
        let mut out = BTreeMap::new();
        out.insert("past".into(), self.past.to_string());
        out.insert("future".into(), self.future.to_string());
        out.insert("actor_past".into(), self.actor_past.to_string());
        out.insert("actor_future".into(), self.actor_future.to_string());
        out.insert("undo_blocked".into(), self.undo_blocked.to_string());
        out.insert("redo_blocked".into(), self.redo_blocked.to_string());
        out.insert(
            "retained_bytes".into(),
            self.retention.retained_bytes.to_string(),
        );
        out.insert(
            "evicted_entries".into(),
            self.retention.evicted_entries.to_string(),
        );
        out.insert(
            "sizing_failures".into(),
            self.retention.sizing_failures.to_string(),
        );
        out.insert("max_entries".into(), self.retention.max_entries.to_string());
        out.insert("max_bytes".into(), self.retention.max_bytes.to_string());
        ToolOutcome::Ok(out)
    }
}

pub fn get_history_depth_snapshot(state: &EditorState) -> GetHistoryDepth {
    let (actor_past, actor_future) = state.history.actor_depths();
    GetHistoryDepth {
        actor_past,
        actor_future,
        undo_blocked: actor_past > 0 && !state.history.can_undo(),
        redo_blocked: actor_future > 0 && !state.history.can_redo(),
        retention: state.history.retention_status(),
        past: state.history.past.len(),
        future: state.history.future.len(),
    }
}

// --- get_viewport ----------------------------------------------------

/// First-party `get_viewport` tool — current pan + zoom state.
pub struct GetViewport {
    pub pan_x: i32,
    pub pan_y: i32,
    pub zoom_percent: i32,
}

impl McpTool for GetViewport {
    fn name(&self) -> &str {
        "get_viewport"
    }
    fn call(&self, _args: &BTreeMap<String, String>) -> ToolOutcome {
        let mut out = BTreeMap::new();
        out.insert("pan_x".into(), self.pan_x.to_string());
        out.insert("pan_y".into(), self.pan_y.to_string());
        out.insert("zoom_percent".into(), self.zoom_percent.to_string());
        ToolOutcome::Ok(out)
    }
}

pub fn get_viewport_snapshot(state: &EditorState) -> GetViewport {
    let v = state.viewport;
    GetViewport {
        pan_x: v.pan_x as i32,
        pan_y: v.pan_y as i32,
        zoom_percent: (v.zoom * 100.0).round() as i32,
    }
}

// --- get_selection_set -----------------------------------------------

/// First-party `get_selection_set` tool — every id in the multi-select
/// set (vs `get_selection` which returns only the anchor).
pub struct GetSelectionSet {
    pub anchor: String,
    pub ids: Vec<String>,
}

impl McpTool for GetSelectionSet {
    fn name(&self) -> &str {
        "get_selection_set"
    }
    fn call(&self, _args: &BTreeMap<String, String>) -> ToolOutcome {
        let encoded = self.ids.join(",");
        let mut out = BTreeMap::new();
        out.insert("count".into(), self.ids.len().to_string());
        out.insert("ids".into(), encoded);
        out.insert("anchor".into(), self.anchor.clone());
        ToolOutcome::Ok(out)
    }
}

pub fn get_selection_set_snapshot(state: &EditorState) -> GetSelectionSet {
    GetSelectionSet {
        anchor: state.selection.anchor.as_str().to_string(),
        ids: state
            .selection
            .set
            .iter()
            .map(|id| id.as_str().to_string())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_bounds_use_resolved_hug_root_and_active_page() {
        let src = r#"{
          "version":"1.0.0","pages":[
            {"id":"hug-page","name":"Hug","children":[
              {"type":"frame","id":"hug","x":10,"y":20,"width":390,
               "height":"fit_content","layout":"vertical","gap":5,"padding":[10,20],
               "children":[
                 {"type":"rectangle","id":"a","width":"fill_container","height":40},
                 {"type":"rectangle","id":"b","width":"fill_container","height":50}
               ]},
              {"type":"frame","id":"fixed","x":500,"y":30,"width":200,"height":20}
            ]},
            {"id":"other-page","name":"Other","children":[
              {"type":"frame","id":"other","x":900,"y":100,"width":20,"height":30}
            ]}
          ],"children":[]
        }"#;
        let parsed = jian_ops_schema::load_str(src).expect("parse fixture");
        let mut state = EditorState::from_document(parsed.value);

        assert_eq!(
            get_canvas_bounds_snapshot(&state).bounds,
            Some((10, 20, 690, 115)),
            "multi-root union must include the layout-resolved Hug height"
        );
        state.ui.active_page_index = 1;
        assert_eq!(
            get_canvas_bounds_snapshot(&state).bounds,
            Some((900, 100, 20, 30)),
            "fixed-size bounds and active-page selection stay unchanged"
        );
    }

    #[test]
    fn compact_canvas_bounds_match_paint_visible_overflow_clips_hidden_and_workspace() {
        let state = EditorState::from_document(jian_ops_schema::load_str(r#"{
          "version":"1.0.0","pages":[{"id":"p","name":"P","children":[
           {"id":"open","type":"frame","x":10.5,"y":20.25,"width":5,"height":5,"clipContent":false,
            "children":[{"id":"escaped","type":"rectangle","x":40,"y":50,"width":10,"height":20}]},
           {"id":"closed","type":"frame","width":5,"height":5,"clipContent":true,
            "children":[{"id":"clipped","type":"rectangle","x":1000,"y":1000,"width":10,"height":20}]},
           {"id":"hidden","type":"rectangle","width":20000,"height":20000,"enabled":false}
          ]},{"id":"other","name":"Other","children":[]}] }"#).unwrap().value);
        for active in [0, 1, usize::MAX] {
            for preserve in [false, true] {
                for workspace in [false, true] {
                    let mut state = state.clone();
                    state.ui.active_page_index = active;
                    state.editor_ui.preserve_authored_geometry = preserve;
                    state.editor_ui.workspace.visible = workspace;
                    let expected = op_pen_loader::editor_state_to_active_page_layout_scene(&state)
                        .content_bounds()
                        .map(|b| {
                            (
                                b.origin.x as i32,
                                b.origin.y as i32,
                                b.size.x as i32,
                                b.size.y as i32,
                            )
                        });
                    assert_eq!(get_canvas_bounds_snapshot(&state).bounds, expected);
                }
            }
        }
    }
}
