//! Paint one borrowed scene branch in its actual ancestor and mask context.

#[path = "selection_lookup.rs"]
mod lookup;
use lookup::{Context, MaskBounds};
pub use lookup::{SceneSelection, SceneSelectionBatch, SceneSelectionCursor};

use super::{
    layer_bounds, mask, paint_node_inner, push_clip_content, Effect, OverlayTransform, PaintCx,
    PaintNodeOptions, Point2D, Rect, SceneNode, MIN_VISIBLE_EFFECT_DEVICE_PX,
};
use crate::ImageBlendMode;

enum Unwind<'a> {
    Ancestor { layers: usize, transform: bool },
    Mask(Option<mask::ActiveMask<'a>>),
}

fn transform(node: &SceneNode) -> Option<OverlayTransform> {
    (node.flip_x || node.flip_y || node.rotation.abs() > f32::EPSILON).then(|| {
        let bounds = node.aggregate_bounds();
        OverlayTransform {
            rotation: node.rotation,
            flip_x: node.flip_x,
            flip_y: node.flip_y,
            pivot: bounds.origin + bounds.size * 0.5,
        }
    })
}

impl SceneSelection<'_> {
    /// Visit only branches that can actually supply paint for this export:
    /// the selected subtree and required sibling mask sources. Ancestors
    /// contribute geometry/composites, never their unrelated own fills.
    pub fn all_paint_sources(&self, mut predicate: impl FnMut(&SceneNode) -> bool) -> bool {
        !self.visible
            || (predicate(self.node)
                && self.contexts.iter().all(|context| match context {
                    Context::Mask(node, _) => predicate(node),
                    Context::Ancestor(_) => true,
                }))
    }

    /// Conservative painted bounds in page coordinates, using the canvas
    /// effect/transform allocator's same geometry. Clips and masks constrain
    /// this branch; no unselected sibling geometry is included.
    pub fn paint_bounds(&self) -> Option<Rect> {
        *self.bounds.get_or_init(|| self.compute_paint_bounds())
    }

    fn compute_paint_bounds(&self) -> Option<Rect> {
        if !self.visible {
            return None;
        }
        let mut transforms = self
            .contexts
            .iter()
            .filter_map(|context| match context {
                Context::Ancestor(node) => transform(node),
                _ => None,
            })
            .collect::<Vec<_>>();
        let mut bounds = layer_bounds::export_source_bounds(self.node, &transforms)?;
        for context in self.contexts.iter().rev() {
            match context {
                Context::Ancestor(node) => {
                    bounds = layer_bounds::export_ancestor_bounds(node, bounds)?;
                    if transform(node).is_some() {
                        transforms.pop();
                    }
                }
                Context::Mask(node, cached) => {
                    // This retained mask link always has the same immutable
                    // outer transform prefix, even for different descendants.
                    let source = cached.get_or_init(|| {
                        if mask::export_mask_supported(node) {
                            MaskBounds::Supported(layer_bounds::export_mask_bounds(
                                node,
                                &transforms,
                            ))
                        } else {
                            MaskBounds::Unsupported
                        }
                    });
                    if let MaskBounds::Supported(source) = source {
                        bounds = layer_bounds::export_intersection(bounds, (*source)?)?;
                    }
                }
            }
        }
        (bounds.size.x > 0.0 && bounds.size.y > 0.0).then_some(bounds)
    }
}

/// Uses the real canvas path/mask painter, with no copied nodes, synthetic
/// parents, overlays, placeholders in mask sources, or unrelated siblings.
/// `cull` is the final page-space export viewport and bounds every layer hint.
pub fn paint_export_selection(cx: &mut PaintCx<'_>, selection: &SceneSelection<'_>, cull: Rect) {
    layer_bounds::with_bounds_reuse(|| paint_export_selection_inner(cx, selection, cull));
}
fn paint_export_selection_inner(cx: &mut PaintCx<'_>, selection: &SceneSelection<'_>, cull: Rect) {
    if !selection.visible {
        return;
    }
    let options = PaintNodeOptions {
        viewport_origin: Point2D::ZERO,
        zoom: 1.0,
        edit_caret: None,
        cull,
        reveals: None,
        hovered: None,
        selected: None,
        pen: None,
        hidden: None,
        now_ms: 0,
        generating_descendant_ids: None,
        generation_accent: None,
        queued_shell_ids: None,
        mask_source: false,
        show_video_badge: false,
        suppress_node_composite_id: None,
        fast_interaction: false,
    };
    let mut transforms = Vec::new();
    let mut unwind = Vec::with_capacity(selection.contexts.len());
    for context in &selection.contexts {
        match context {
            Context::Mask(node, _) => {
                let bounds = layer_bounds::export_layer_bounds(cull, &transforms);
                unwind.push(Unwind::Mask(mask::begin_export_mask(
                    cx, node, &options, bounds,
                )));
            }
            Context::Ancestor(node) => {
                let transformed = transform(node);
                if let Some(t) = transformed {
                    crate::widgets::canvas_overlay_transform::replay_on_backend(
                        cx,
                        std::slice::from_ref(&t),
                    );
                    transforms.push(t);
                }
                let opacity = if node.composite_opacity.is_finite() {
                    node.composite_opacity.clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let mode = if mask::mask_type(node).is_some() {
                    ImageBlendMode::Normal
                } else {
                    node.blend_mode
                };
                let mut layers = 0;
                if opacity < 1.0 || mode != ImageBlendMode::Normal {
                    cx.backend.push_composite_layer(
                        layer_bounds::export_layer_bounds(cull, &transforms),
                        opacity,
                        mode,
                    );
                    layers += 1;
                }
                let blur = node
                    .effects
                    .iter()
                    .find_map(|effect| match effect {
                        Effect::Blur(blur) if blur.radius > 0.0 => Some(blur.radius * 0.5),
                        _ => None,
                    })
                    .filter(|sigma| sigma * cx.backend.dpi_scale() >= MIN_VISIBLE_EFFECT_DEVICE_PX);
                if let Some(sigma) = blur {
                    cx.backend.push_blur_layer(sigma);
                    layers += 1;
                }
                if push_clip_content(cx, node, node.bounds, 1.0) {
                    layers += 1;
                }
                unwind.push(Unwind::Ancestor {
                    layers,
                    transform: transformed.is_some(),
                });
            }
        }
    }
    let _ = paint_node_inner(cx, selection.node, &options, &mut transforms, false);
    for item in unwind.into_iter().rev() {
        match item {
            Unwind::Mask(active) => mask::finish_active_mask(cx, active, &options, &mut transforms),
            Unwind::Ancestor { layers, transform } => {
                for _ in 0..layers {
                    cx.backend.restore();
                }
                if transform {
                    cx.backend.restore();
                    transforms.pop();
                }
            }
        }
    }
}
