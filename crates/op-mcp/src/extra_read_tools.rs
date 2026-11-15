//! Read-tool additions that didn't fit in `tools.rs`. Each tool is an
//! `EditorState` snapshot taken at registration time, mirroring the
//! spine in `tools.rs`.

use std::collections::BTreeMap;

use jian_ops_schema::node::PenNode;
use op_editor_core::geometry::aggregate_bounds;
use op_editor_core::pen_node_ext::PenNodeExt;
use op_editor_core::EditorState;

use super::tools::kind_label;
use super::{McpTool, ToolErrorCode, ToolOutcome};

/// Immediate-children listing for a single container node, keyed by
/// parent id. Built at snapshot time so the registered tool stays
/// `&self`-only (same discipline as `GetNode`).
///
/// `known_ids` tracks every node id that exists in the document so
/// callers can tell an unknown id apart from a known node that just
/// has no children — `count=0` for the latter, `ToolFailed` for the
/// former.
pub struct GetNodeChildren {
    pub children: BTreeMap<String, Vec<ChildRecord>>,
    pub known_ids: std::collections::BTreeSet<String>,
}

#[derive(Debug, Clone)]
pub struct ChildRecord {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl McpTool for GetNodeChildren {
    fn name(&self) -> &str {
        "get_node_children"
    }
    fn call(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        let raw = match args.get("node_id") {
            Some(s) => s,
            None => {
                return ToolOutcome::Err(
                    ToolErrorCode::MissingArgument,
                    "node_id is required".into(),
                );
            }
        };
        let id: &str = raw.as_str();
        if !self.known_ids.contains(id) {
            return ToolOutcome::Err(ToolErrorCode::ToolFailed, format!("node {id} not found"));
        }
        // Empty children + known id ⇒ count=0 (NOT an error).
        let records = self.children.get(id).map(Vec::as_slice).unwrap_or_default();
        let mut out = BTreeMap::new();
        out.insert("count".into(), records.len().to_string());
        let ids: Vec<&str> = records.iter().map(|r| r.id.as_str()).collect();
        out.insert("ids".into(), ids.join(","));
        for (i, rec) in records.iter().enumerate() {
            let prefix = format!("child_{i}");
            out.insert(format!("{prefix}_id"), rec.id.to_string());
            out.insert(format!("{prefix}_kind"), rec.kind.clone());
            out.insert(format!("{prefix}_name"), rec.name.clone());
            out.insert(format!("{prefix}_x"), rec.x.to_string());
            out.insert(format!("{prefix}_y"), rec.y.to_string());
            out.insert(format!("{prefix}_width"), rec.width.to_string());
            out.insert(format!("{prefix}_height"), rec.height.to_string());
        }
        ToolOutcome::Ok(out)
    }
}

pub fn get_node_children_snapshot(state: &EditorState) -> GetNodeChildren {
    let mut children: BTreeMap<String, Vec<ChildRecord>> = BTreeMap::new();
    let mut known_ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    match state.doc.pages.as_ref() {
        Some(pages) => {
            for page in pages {
                walk(&page.children, &mut children, &mut known_ids);
            }
        }
        None => walk(&state.doc.children, &mut children, &mut known_ids),
    }
    GetNodeChildren {
        children,
        known_ids,
    }
}

/// Capture only the requested immediate-child records. The original map is
/// built postorder and only nonempty containers overwrite entries: later leaves
/// or empty containers with the same ID must not erase an earlier child list.
pub fn get_node_children_snapshot_for_call(
    state: &EditorState,
    args: &BTreeMap<String, String>,
) -> GetNodeChildren {
    fn last_children<'a>(
        roots: &'a [PenNode],
        id: &str,
        known: &mut bool,
        found: &mut Option<&'a [PenNode]>,
    ) {
        let mut nodes = roots.iter();
        let mut ancestors = Vec::new();
        loop {
            let Some(node) = nodes.next() else {
                let Some(parent) = ancestors.pop() else {
                    return;
                };
                nodes = parent;
                continue;
            };
            let children = node.children().filter(|children| !children.is_empty());
            if node.id_str() == id {
                *known = true;
                if let Some(children) = children {
                    *found = Some(children.as_slice());
                }
                // This matching parent's nonempty list overwrites all its
                // descendants in the legacy postorder map. Skip that subtree;
                // later siblings/pages must still be checked for duplicate IDs.
                continue;
            }
            if let Some(children) = children {
                ancestors.push(nodes);
                nodes = children.iter();
            }
        }
    }
    let mut children = BTreeMap::new();
    let mut known_ids = std::collections::BTreeSet::new();
    if let Some(id) = args.get("node_id") {
        if let Some((known, found)) = super::trusted_reads::global_children(state, id) {
            if known {
                known_ids.insert(id.clone());
            }
            if let Some(found) = found {
                children.insert(id.clone(), found.iter().map(child_record).collect());
            }
            return GetNodeChildren {
                children,
                known_ids,
            };
        }
        let mut known = false;
        let mut found = None;
        match &state.doc.pages {
            Some(pages) => {
                for page in pages {
                    last_children(&page.children, id, &mut known, &mut found);
                }
            }
            None => last_children(&state.doc.children, id, &mut known, &mut found),
        }
        if known {
            known_ids.insert(id.clone());
        }
        if let Some(found) = found {
            children.insert(id.clone(), found.iter().map(child_record).collect());
        }
    }
    GetNodeChildren {
        children,
        known_ids,
    }
}

fn child_record(child: &PenNode) -> ChildRecord {
    let b = aggregate_bounds(child);
    ChildRecord {
        id: child.id_str().into(),
        kind: kind_label(child).into(),
        name: child.base().name.clone().unwrap_or_default(),
        x: b.x as i32,
        y: b.y as i32,
        width: b.w as i32,
        height: b.h as i32,
    }
}

fn walk(
    roots: &[PenNode],
    out: &mut BTreeMap<String, Vec<ChildRecord>>,
    known: &mut std::collections::BTreeSet<String>,
) {
    let mut nodes = roots.iter();
    let mut ancestors: Vec<(std::slice::Iter<'_, PenNode>, &PenNode)> = Vec::new();
    loop {
        let Some(node) = nodes.next() else {
            let Some((siblings, parent)) = ancestors.pop() else {
                return;
            };
            // Install only after the children, retaining the original last
            // postorder nonempty-container winner for duplicate authored IDs.
            out.insert(
                parent.id_str().to_owned(),
                parent
                    .children()
                    .expect("entered nonempty children")
                    .iter()
                    .map(child_record)
                    .collect(),
            );
            nodes = siblings;
            continue;
        };
        known.insert(node.id_str().to_owned());
        if let Some(children) = node.children().filter(|children| !children.is_empty()) {
            ancestors.push((nodes, node));
            nodes = children.iter();
        }
    }
}
