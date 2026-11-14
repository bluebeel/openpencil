//! Resolve requested batch identities once and stream complete files in order.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use op_editor_ui::layout_scene::{LayoutScene, SceneNode, ScenePage};

use crate::{ExportError, RasterFormat};

#[derive(Debug, Clone)]
pub struct RasterFileTarget {
    pub node_id: String,
    pub path: PathBuf,
}

pub fn export_scene_files(
    scene: &LayoutScene,
    targets: &[RasterFileTarget],
    format: RasterFormat,
    scale: f32,
) -> Vec<Result<(), ExportError>> {
    let Some(page) = scene.active_page() else {
        return targets
            .iter()
            .map(|_| Err(ExportError::NoActivePage))
            .collect();
    };
    let nodes = requested_nodes(page, targets);
    targets
        .iter()
        .map(|target| {
            let node = nodes.get(target.node_id.as_str()).ok_or_else(|| {
                ExportError::NodeNotFoundOnActivePage {
                    node_id: target.node_id.clone(),
                }
            })?;
            crate::export_scene_node_raster(node, &target.path, format, scale)
        })
        .collect()
}

fn requested_nodes<'a>(
    page: &'a ScenePage,
    targets: &[RasterFileTarget],
) -> HashMap<&'a str, &'a SceneNode> {
    let wanted: HashSet<&str> = targets
        .iter()
        .map(|target| target.node_id.as_str())
        .collect();
    let mut found = HashMap::with_capacity(wanted.len());
    if wanted.is_empty() {
        return found;
    }
    // Iterator frames use depth-sized scratch, not a copy of every sibling.
    let mut stack = vec![page.children.iter()];
    while let Some(children) = stack.last_mut() {
        let Some(node) = children.next() else {
            stack.pop();
            continue;
        };
        if wanted.contains(node.id.as_str()) {
            found.entry(node.id.as_str()).or_insert(node);
            if found.len() == wanted.len() {
                break;
            }
        }
        if !node.children.is_empty() {
            stack.push(node.children.iter());
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use op_editor_ui::layout_scene::NodeKind;

    #[test]
    fn batch_lookup_preserves_nested_duplicate_and_missing_id_semantics() {
        let mut first = SceneNode::leaf("first", NodeKind::Rect);
        first
            .children
            .push(SceneNode::leaf("duplicate", NodeKind::Rect));
        let page = ScenePage {
            id: "page".into(),
            name: "Page".into(),
            children: vec![first, SceneNode::leaf("duplicate", NodeKind::Rect)],
        };
        let targets: Vec<_> = ["duplicate", "absent", "first", "duplicate"]
            .into_iter()
            .map(|id| RasterFileTarget {
                node_id: id.into(),
                path: format!("{id}.png").into(),
            })
            .collect();
        let found = requested_nodes(&page, &targets);
        for target in &targets {
            assert_eq!(
                found
                    .get(target.node_id.as_str())
                    .copied()
                    .map(std::ptr::from_ref),
                page.find(target.node_id.as_str()).map(std::ptr::from_ref)
            );
        }
        assert_eq!(found.len(), 2);
    }
}
