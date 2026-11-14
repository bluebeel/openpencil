//! Request-scoped SVG targets, resolved by one borrowed preorder walk.

use std::collections::{HashMap, HashSet};

use super::{
    context::AncestorEffect,
    masks, serialize_node_context,
    transform::{node_transform, AncestorClip},
    SceneNode, ScenePage, SvgExportError,
};

struct Target<'a> {
    node: &'a SceneNode,
    ancestor_xform: glam::Affine2,
    context_head: Option<usize>,
}
struct ContextLink<'a> {
    effect: AncestorEffect<'a>,
    parent: Option<usize>,
}
struct ActiveContext<'a> {
    effect: AncestorEffect<'a>,
    retained: Option<usize>,
}
struct Frame<'a> {
    nodes: &'a [SceneNode],
    at: usize,
    next_mask: Option<usize>,
    mask_installed: bool,
    ancestor_xform: glam::Affine2,
    base_context_depth: usize,
    restore_context_depth: usize,
}

impl<'a> Frame<'a> {
    fn new(nodes: &'a [SceneNode], xform: glam::Affine2, base: usize, restore: usize) -> Self {
        Self {
            nodes,
            at: 0,
            next_mask: nodes.iter().position(|node| masks::marker(node).is_some()),
            mask_installed: false,
            ancestor_xform: xform,
            base_context_depth: base,
            restore_context_depth: restore,
        }
    }
}

/// Immutable requested nodes and the union of their inherited context chains.
/// Nodes/resources stay borrowed; sibling targets share retained prefix links.
/// Duplicate scene IDs retain the first preorder hit. Requests may repeat IDs.
pub struct SvgNodeBatch<'a> {
    targets: HashMap<&'a str, Target<'a>>,
    contexts: Vec<ContextLink<'a>>,
}

impl<'a> SvgNodeBatch<'a> {
    pub fn new(page: &'a ScenePage, node_ids: &[String]) -> Self {
        let mut batch = Self {
            targets: HashMap::new(),
            contexts: Vec::new(),
        };
        let mut wanted = node_ids.iter().map(String::as_str).collect::<HashSet<_>>();
        let mut active = Vec::<ActiveContext<'a>>::new();
        let mut stack = vec![Frame::new(&page.children, glam::Affine2::IDENTITY, 0, 0)];
        while !wanted.is_empty() {
            let Some(frame) = stack.last_mut() else { break };
            let Some(node) = frame.nodes.get(frame.at) else {
                let depth = frame.restore_context_depth;
                stack.pop();
                active.truncate(depth);
                continue;
            };
            let index = frame.at;
            frame.at += 1;
            let ancestor_xform = frame.ancestor_xform;
            if frame.next_mask == Some(index) {
                // Every marker ends the previous front run, including hidden
                // or unsupported masks. The marker itself is never masked.
                active.truncate(frame.base_context_depth);
                frame.next_mask = frame.nodes[frame.at..]
                    .iter()
                    .position(|node| masks::marker(node).is_some())
                    .map(|offset| frame.at + offset);
                frame.mask_installed = false;
            } else if !frame.mask_installed {
                if let Some(mask) = frame
                    .next_mask
                    .map(|index| &frame.nodes[index])
                    .filter(|node| masks::operation(node).is_some())
                {
                    active.push(ActiveContext {
                        effect: AncestorEffect::Mask {
                            node: mask,
                            parent_xform: ancestor_xform,
                        },
                        retained: None,
                    });
                }
                frame.mask_installed = true;
            }
            if wanted.remove(node.id.as_str()) {
                let mut parent = None;
                for context in &mut active {
                    let at = if let Some(at) = context.retained {
                        at
                    } else {
                        let at = batch.contexts.len();
                        batch.contexts.push(ContextLink {
                            effect: context.effect.clone(),
                            parent,
                        });
                        context.retained = Some(at);
                        at
                    };
                    parent = Some(at);
                }
                batch.targets.insert(
                    node.id.as_str(),
                    Target {
                        node,
                        ancestor_xform,
                        context_head: parent,
                    },
                );
                if wanted.is_empty() {
                    break;
                }
            }
            if node.children.is_empty() {
                continue;
            }
            let child_xform = ancestor_xform * node_transform(node);
            let depth = active.len();
            if let Some(effect) = AncestorEffect::composite(node, masks::marker(node).is_some()) {
                active.push(ActiveContext {
                    effect,
                    retained: None,
                });
            }
            if node.clip_content && node.bounds.size.x != 0.0 && node.bounds.size.y != 0.0 {
                active.push(ActiveContext {
                    effect: AncestorEffect::Clip(AncestorClip::for_node(node, child_xform)),
                    retained: None,
                });
            }
            stack.push(Frame::new(&node.children, child_xform, active.len(), depth));
        }
        batch
    }

    /// Only the current SVG and its inherited metadata are temporary; source
    /// subtrees and mask resources remain borrowed until this batch is dropped.
    pub fn serialize(&self, node_id: &str, scale: f32) -> Result<String, SvgExportError> {
        let target = self
            .targets
            .get(node_id)
            .ok_or_else(|| SvgExportError::NodeNotFound(node_id.to_owned()))?;
        let mut head = target.context_head;
        let mut effects = Vec::new();
        while let Some(index) = head {
            let link = &self.contexts[index];
            effects.push(link.effect.clone());
            head = link.parent;
        }
        effects.reverse();
        serialize_node_context(target.node, node_id, target.ancestor_xform, &effects, scale)
    }
}
