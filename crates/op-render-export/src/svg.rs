//! SVG file export wrapper.
//!
//! The platform-neutral serializer lives in `op_editor_ui::svg_export`
//! so desktop and web export the same vector markup. This module keeps
//! the desktop API focused on writing the generated SVG to disk.

use op_editor_ui::layout_scene::{LayoutScene, ScenePage};
use std::path::Path as StdPath;

use crate::ExportError;

/// Serialize the active page to UTF-8 SVG bytes.
pub fn render_svg_bytes(scene: &LayoutScene) -> Result<Vec<u8>, ExportError> {
    op_editor_ui::svg_export::serialize_active_page_svg(scene)
        .map(String::into_bytes)
        .map_err(|e| ExportError::SvgSerialize(e.to_string()))
}

/// Serialize one node and its subtree to UTF-8 SVG bytes.
pub fn render_node_svg_bytes(scene: &LayoutScene, node_id: &str) -> Result<Vec<u8>, ExportError> {
    op_editor_ui::svg_export::serialize_node_svg(scene, node_id)
        .map(String::into_bytes)
        .map_err(|e| ExportError::SvgSerialize(e.to_string()))
}

/// Serialize one borrowed page, sharing the already resolved scene.
pub fn render_page_svg_bytes(page: &ScenePage, scale: f32) -> Result<Vec<u8>, ExportError> {
    op_editor_ui::svg_export::serialize_page_svg(page, scale)
        .map(String::into_bytes)
        .map_err(|e| ExportError::SvgSerialize(e.to_string()))
}

/// Serialize a node on an explicitly resolved page, including ancestor clips.
pub fn render_node_on_page_svg_bytes(
    page: &ScenePage,
    node_id: &str,
    scale: f32,
) -> Result<Vec<u8>, ExportError> {
    op_editor_ui::svg_export::serialize_node_on_page_svg(page, node_id, scale)
        .map(String::into_bytes)
        .map_err(|e| ExportError::SvgSerialize(e.to_string()))
}

/// Borrowed, request-scoped SVG lookup shared across a multi-node export.
/// Payloads are serialized one at a time; this owns no scene or SVG buffers.
pub struct NodeSvgBatch<'a> {
    prepared: op_editor_ui::svg_export::SvgNodeBatch<'a>,
}
impl<'a> NodeSvgBatch<'a> {
    pub fn new(page: &'a ScenePage, node_ids: &[String]) -> Self {
        Self {
            prepared: op_editor_ui::svg_export::SvgNodeBatch::new(page, node_ids),
        }
    }
    pub fn render(&self, node_id: &str, scale: f32) -> Result<Vec<u8>, ExportError> {
        self.prepared
            .serialize(node_id, scale)
            .map(String::into_bytes)
            .map_err(|e| ExportError::SvgSerialize(e.to_string()))
    }
    /// Deliver one complete target before serializing the next; context
    /// allocation and immutable duplicate bounds are reused by this batch.
    pub fn render_each(
        &self,
        ids: &[String],
        scale: f32,
        mut consume: impl FnMut(&str, Result<Vec<u8>, ExportError>) -> bool,
    ) {
        self.prepared.serialize_each(ids, scale, |id, result| {
            consume(
                id,
                result
                    .map(String::into_bytes)
                    .map_err(|error| ExportError::SvgSerialize(error.to_string())),
            )
        });
    }
}

/// Serialize the scene's active page to an SVG file at `target`.
///
/// `serialize_active_page_svg` lives in `op_editor_ui`, a crate this pass
/// does not own, and still reports `String`; [`ExportError::SvgSerialize`]
/// carries its sentence verbatim (`to_string` so the adapter keeps working
/// if that crate later types its own error).
pub fn export_svg(scene: &LayoutScene, target: &StdPath) -> Result<(), ExportError> {
    std::fs::write(target, render_svg_bytes(scene)?).map_err(|e| ExportError::Write(e.to_string()))
}

/// Serialize one node and its subtree to an SVG file at `target`. Same
/// upstream-`String` adapter rationale as [`export_svg`].
pub fn export_node_svg(
    scene: &LayoutScene,
    node_id: &str,
    target: &StdPath,
) -> Result<(), ExportError> {
    std::fs::write(target, render_node_svg_bytes(scene, node_id)?)
        .map_err(|e| ExportError::Write(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{filled_rect, scene_with};
    use op_editor_ui::Color;

    #[test]
    fn export_svg_writes_rect_element() {
        let scene = scene_with(vec![filled_rect(
            "r1",
            5.0,
            5.0,
            120.0,
            60.0,
            Color {
                r: 0.2,
                g: 0.4,
                b: 0.6,
                a: 1.0,
            },
        )]);
        let tmp = std::env::temp_dir().join(format!("op-export-svg-{}.svg", std::process::id()));
        let res = export_svg(&scene, &tmp);
        assert!(res.is_ok(), "export_svg failed: {res:?}");
        let body = std::fs::read_to_string(&tmp).unwrap();
        assert!(body.starts_with("<svg "), "missing svg root: {body}");
        assert!(body.contains("<rect "), "missing rect element: {body}");
        assert!(body.contains(r#"width="120""#), "rect width wrong: {body}");
        assert!(body.ends_with("</svg>"), "missing svg close: {body}");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn export_svg_fails_on_empty_scene() {
        let scene = scene_with(Vec::new());
        let tmp = std::env::temp_dir().join(format!("op-svg-empty-{}.svg", std::process::id()));
        let res = export_svg(&scene, &tmp);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().to_string(), "nothing to export");
    }

    #[test]
    fn export_node_svg_writes_only_selected_subtree() {
        let scene = scene_with(vec![
            filled_rect("selected", 5.0, 5.0, 120.0, 60.0, Color::RED),
            filled_rect("sibling", 500.0, 500.0, 10.0, 10.0, Color::BLACK),
        ]);
        let tmp =
            std::env::temp_dir().join(format!("op-export-node-svg-{}.svg", std::process::id()));

        export_node_svg(&scene, "selected", &tmp).expect("selected svg export");

        let body = std::fs::read_to_string(&tmp).unwrap();
        assert!(body.contains("selected"), "{body}");
        assert!(!body.contains("sibling"), "{body}");
        let _ = std::fs::remove_file(&tmp);
    }
}
