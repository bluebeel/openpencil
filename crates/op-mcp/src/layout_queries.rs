//! Request-bound computed geometry; never retains an EditorState clone.
use super::*;
use std::collections::HashSet;

struct PageScope<'a> {
    index: usize,
    id: &'a str,
    roots: &'a [PenNode],
}

fn page_scope<'a>(
    state: &'a EditorState,
    args: &BTreeMap<String, String>,
) -> Result<PageScope<'a>, ToolCallError> {
    let requested = arg_alias(args, &["pageId", "page_id", "page"]);
    match state.doc.pages.as_ref().filter(|pages| !pages.is_empty()) {
        Some(pages) => {
            let active = state.ui.active_page_index.min(pages.len() - 1);
            let id = requested.unwrap_or(&pages[active].id);
            pages
                .iter()
                .enumerate()
                .find(|(_, page)| page.id == id)
                .map(|(index, page)| PageScope {
                    index,
                    id: &page.id,
                    roots: &page.children,
                })
                .ok_or_else(|| (ToolErrorCode::ToolFailed, format!("page not found: {id}")))
        }
        None => match requested {
            None | Some("0") => Ok(PageScope {
                index: 0,
                id: "0",
                roots: &state.doc.children,
            }),
            Some(id) => Err((ToolErrorCode::ToolFailed, format!("page not found: {id}"))),
        },
    }
}

fn prepare_layout(
    state: &EditorState,
    args: &BTreeMap<String, String>,
) -> Result<ToolOutcome, ToolCallError> {
    // Preserve page-before-depth error precedence without constructing any scene.
    let page = page_scope(state, args)?;
    let depth = arg_alias(args, &["maxDepth", "depth"]).map_or(Ok(1), |raw| {
        raw.parse::<u32>().map_err(|_| {
            (
                ToolErrorCode::InvalidArgument,
                format!("maxDepth must be a u32, got {raw:?}"),
            )
        })
    })?;
    let parent = arg_alias(args, &["parentId", "parent_id", "parent"]);
    let scene = op_pen_loader::editor_state_to_page_item_layout_scene(state, page.index, parent);
    let roots = scene
        .pages
        .get(page.index)
        .map_or(&[][..], |page| page.children.as_slice());
    let records = limited_records(roots, page.roots, parent, depth);
    Ok(snapshot_layout_outcome(
        &PageLayout {
            id: page.id.into(),
            records,
        },
        args,
    ))
}

/// Build the response while borrowing the canonical document. Geometry includes
/// the complete containing root, preserving flex siblings and ancestor transforms.
pub fn snapshot_layout_snapshot_for_call(
    state: &EditorState,
    args: &BTreeMap<String, String>,
) -> SnapshotLayout {
    let prepared = prepare_layout(state, args)
        .unwrap_or_else(|(code, message)| ToolOutcome::Err(code, message));
    SnapshotLayout {
        state: None,
        prepared: Some(prepared),
    }
}

fn limited_records(
    roots: &[SceneNode],
    raw: &[PenNode],
    parent: Option<&str>,
    max_depth: u32,
) -> Vec<LayoutRecord> {
    fn first<'a>(nodes: &'a [SceneNode], id: &str, depth: u32) -> Option<(&'a SceneNode, u32)> {
        nodes.iter().find_map(|node| {
            if node.id == id {
                Some((node, depth))
            } else {
                first(&node.children, id, depth + 1)
            }
        })
    }
    let parent_node = parent.and_then(|id| first(roots, id, 0));
    if parent.is_some() && parent_node.is_none() {
        return Vec::new();
    }
    let cutoff = parent_node.map_or(max_depth, |(_, depth)| depth + max_depth + 1);
    fn walk(
        nodes: &[SceneNode],
        ancestors: &mut Vec<String>,
        depth: u32,
        cutoff: u32,
        parent: Option<&str>,
        first_parent: Option<&SceneNode>,
        out: &mut Vec<LayoutRecord>,
    ) {
        if depth > cutoff {
            return;
        }
        for node in nodes {
            if parent.is_none()
                || ancestors.iter().any(|id| Some(id.as_str()) == parent)
                || first_parent.is_some_and(|first| std::ptr::eq(node, first))
            {
                let bounds = node.aggregate_bounds();
                out.push(LayoutRecord {
                    id: node.id.clone(),
                    name: None,
                    type_label: scene_kind_label(&node.kind).into(),
                    ancestors: ancestors.clone(),
                    depth,
                    x: f64::from(bounds.origin.x),
                    y: f64::from(bounds.origin.y),
                    w: f64::from(bounds.size.x),
                    h: f64::from(bounds.size.y),
                });
            }
            if !node.children.is_empty() && depth < cutoff {
                ancestors.push(node.id.clone());
                walk(
                    &node.children,
                    ancestors,
                    depth + 1,
                    cutoff,
                    parent,
                    first_parent,
                    out,
                );
                ancestors.pop();
            }
        }
    }
    let mut records = Vec::new();
    walk(
        roots,
        &mut Vec::new(),
        0,
        cutoff,
        parent,
        parent_node.map(|(node, _)| node),
        &mut records,
    );
    let ids: HashSet<&str> = records.iter().map(|record| record.id.as_str()).collect();
    fn metadata<'a>(
        nodes: &'a [PenNode],
        wanted: &HashSet<&str>,
        out: &mut HashMap<&'a str, &'a PenNode>,
    ) {
        for node in nodes {
            if wanted.contains(node.id_str()) {
                out.insert(node.id_str(), node);
            }
            if let Some(children) = node.children() {
                metadata(children, wanted, out);
            }
        }
    }
    let mut meta = HashMap::new();
    metadata(raw, &ids, &mut meta);
    for record in &mut records {
        if let Some(node) = meta.get(record.id.as_str()) {
            record.name = node.base().name.clone();
            record.type_label = kind_label(node).into();
        }
    }
    records
}

fn prepare_empty_space(
    state: &EditorState,
    args: &BTreeMap<String, String>,
) -> Result<ToolOutcome, ToolCallError> {
    required_i32_arg(args, "width")?;
    required_i32_arg(args, "height")?;
    optional_i32_arg(args, "padding", 50)?;
    if !args.contains_key("direction") {
        return Err((
            ToolErrorCode::MissingArgument,
            "direction is required".into(),
        ));
    }
    let page = page_scope(state, args)?;
    let node_id = arg_alias(args, &["nodeId", "node_id", "node"]);
    let scene = op_pen_loader::editor_state_to_page_item_layout_scene(state, page.index, node_id);
    let scene_page = scene.pages.get(page.index);
    let (roots, all) = match node_id {
        Some(id) => (
            Vec::new(),
            scene_page
                .and_then(|page| page.find(id))
                .map(bounds_record)
                .into_iter()
                .collect(),
        ),
        None => (
            scene_page
                .into_iter()
                .flat_map(|page| &page.children)
                .map(bounds_record)
                .collect(),
            Vec::new(),
        ),
    };
    Ok(FindEmptySpace {
        pages: vec![PageSpace {
            id: page.id.into(),
            roots,
            all,
        }],
        active_page_id: page.id.into(),
        prepared: None,
    }
    .call(args))
}

/// Retain only requested computed bounds, rather than every page's descendants.
pub fn find_empty_space_snapshot_for_call(
    state: &EditorState,
    args: &BTreeMap<String, String>,
) -> FindEmptySpace {
    let prepared = prepare_empty_space(state, args)
        .unwrap_or_else(|(code, message)| ToolOutcome::Err(code, message));
    FindEmptySpace {
        pages: Vec::new(),
        active_page_id: String::new(),
        prepared: Some(prepared),
    }
}

#[cfg(test)]
#[path = "layout_queries_tests.rs"]
mod tests;
