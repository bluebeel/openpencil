//! Borrowed inherited composite/clip/mask context for selected-node exports.

use super::transform::AncestorClip;
use super::*;
use crate::ImageBlendMode;
use std::sync::{Arc, OnceLock};

#[derive(Clone)]
pub(super) enum AncestorEffect<'a> {
    Clip(AncestorClip),
    Mask {
        node: &'a SceneNode,
        parent_xform: glam::Affine2,
        bounds: Arc<OnceLock<Option<Rect>>>,
    },
    Composite {
        opacity: f32,
        mode: ImageBlendMode,
    },
}

impl AncestorEffect<'_> {
    pub(super) fn composite(node: &SceneNode, mask_source: bool) -> Option<Self> {
        let mode = if mask_source {
            ImageBlendMode::Normal
        } else {
            node.blend_mode
        };
        let opacity = if node.composite_opacity.is_finite() {
            node.composite_opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        (mode != ImageBlendMode::Normal || opacity < 1.0)
            .then_some(Self::Composite { opacity, mode })
    }
}

pub(super) fn apply_bounds(mut bounds: Rect, effects: &[AncestorEffect<'_>]) -> Option<Rect> {
    for effect in effects {
        bounds = match effect {
            AncestorEffect::Clip(clip) => {
                transform::apply_ancestor_clip_bounds(bounds, std::slice::from_ref(clip))?
            }
            AncestorEffect::Mask {
                node,
                parent_xform,
                bounds: cached,
            } => transform::intersect(
                bounds,
                (*cached.get_or_init(|| masks::source_bounds(node, *parent_xform)))?,
            )?,
            AncestorEffect::Composite { .. } => bounds,
        };
    }
    Some(bounds)
}

pub(super) fn emit_defs(out: &mut String, effects: &[AncestorEffect<'_>]) {
    for effect in effects {
        match effect {
            AncestorEffect::Clip(clip) => {
                transform::emit_ancestor_clip_defs(out, std::slice::from_ref(clip))
            }
            AncestorEffect::Mask {
                node, parent_xform, ..
            } => {
                let _coverage = text_layers::coverage::Scope::world(*parent_xform);
                // The inherited mask is a sibling, not part of the selected
                // subtree. Retain its actual paint resources and descendants.
                emit_node_defs(out, node, false);
                masks::emit_definition(out, node, *parent_xform, true);
            }
            AncestorEffect::Composite { .. } => {}
        }
    }
}

pub(super) fn emit_groups_start(out: &mut String, effects: &[AncestorEffect<'_>]) {
    for effect in effects {
        match effect {
            AncestorEffect::Clip(clip) => {
                transform::emit_ancestor_clip_groups_start(out, std::slice::from_ref(clip))
            }
            AncestorEffect::Mask { node, .. } => {
                if let Some(operation) = masks::operation(node) {
                    masks::emit_group_start(out, node, operation, true);
                }
            }
            AncestorEffect::Composite { opacity, mode } => {
                render::emit_composite_start(out, *opacity, *mode);
            }
        }
    }
}

pub(super) fn close_groups(out: &mut String, effects: &[AncestorEffect<'_>]) {
    for _ in effects {
        out.push_str("</g>");
    }
}
