//! Conservative readiness for exactly the image paint resources a branch may
//! use. No fetch/decode starts here. A miss retains the complete original pump.
use op_editor_ui::layout_scene::{stable_image_source_id, NodeKind, SceneFillLayer, SceneNode};
use op_host_native::NativeBackend;

#[path = "image_resource_plan.rs"]
mod resource_plan;
pub(super) fn with_scene<R>(
    scene: &std::sync::Arc<op_editor_ui::layout_scene::LayoutScene>,
    action: impl FnOnce() -> R,
) -> R {
    resource_plan::with_scene(scene, action)
}

fn source_ready(backend: &mut NativeBackend, id: u64, source: &str) -> bool {
    let id = if id == 0 {
        stable_image_source_id(source)
    } else {
        id
    };
    // Preserve the export's full-resolution requirement, not merely a coarse
    // resident preview. Encoded bytes must also be available, since the real
    // painter consults that cache before using the native raster.
    id_ready(backend, id)
}
fn id_ready(backend: &mut NativeBackend, id: u64) -> bool {
    backend.image_decoded(id, &[], u32::MAX)
        && op_editor_ui::widgets::canvas_viewport_image::cached_bytes_for(id).is_some()
}
pub(super) fn nodes_ready(backend: &mut NativeBackend, nodes: &[SceneNode]) -> bool {
    nodes.iter().all(|node| {
        if let Some(plan) = resource_plan::for_node(node) {
            if let Some(ids) = plan.ids() {
                return ids.iter().all(|id| id_ready(backend, *id));
            }
        }
        nodes_ready_uncached(backend, std::slice::from_ref(node))
    })
}
fn nodes_ready_uncached(backend: &mut NativeBackend, nodes: &[SceneNode]) -> bool {
    nodes.iter().all(|node| {
        // Custom/widget painters may acquire resources not represented by the
        // ordinary scene image fields. Keep their original discovery path.
        if node.widget.is_some() || matches!(node.kind, NodeKind::Other(_)) {
            return false;
        }
        if let Some(source) = &node.image_src {
            if !source_ready(backend, node.image_src_id, source) {
                return false;
            }
        }
        if let Some(source) = &node.image_thumbnail_src {
            if !source_ready(backend, node.image_thumbnail_src_id, source) {
                return false;
            }
        }
        for layer in &node.fill_layers {
            if let SceneFillLayer::Image {
                src,
                src_id,
                thumbnail_src,
                thumbnail_src_id,
                ..
            } = layer
            {
                if !source_ready(backend, *src_id, src) {
                    return false;
                }
                if let Some(source) = thumbnail_src {
                    if !source_ready(backend, *thumbnail_src_id, source) {
                        return false;
                    }
                }
            }
        }
        nodes_ready_uncached(backend, &node.children)
    })
}
