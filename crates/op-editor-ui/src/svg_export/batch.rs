//! Request-scoped SVG targets, resolved by one borrowed preorder walk.

use std::{
    collections::{HashMap, HashSet},
    slice,
};

use super::{
    serialize_node_context,
    transform::{node_transform, AncestorClip},
    SceneNode, ScenePage, SvgExportError,
};

struct Target<'a> {
    node: &'a SceneNode,
    ancestor_xform: glam::Affine2,
    clip_head: Option<usize>,
}
struct ClipLink {
    clip: AncestorClip,
    parent: Option<usize>,
}
struct ActiveClip<'a> {
    node: &'a SceneNode,
    xform: glam::Affine2,
    retained: Option<usize>,
}
struct Frame<'a> {
    nodes: slice::Iter<'a, SceneNode>,
    ancestor_xform: glam::Affine2,
    restore_clip_depth: usize,
}

/// An immutable view of the requested nodes on one resolved page. Missing IDs
/// remain per-entry errors; duplicate scene IDs retain the first preorder hit,
/// matching single-node SVG export. Duplicate requested entries are permitted.
/// No nodes, subtrees, typefaces, assets or output SVG strings are retained.
pub struct SvgNodeBatch<'a> {
    targets: HashMap<&'a str, Target<'a>>,
    clips: Vec<ClipLink>,
}

impl<'a> SvgNodeBatch<'a> {
    pub fn new(page: &'a ScenePage, node_ids: &[String]) -> Self {
        let mut batch = Self {
            targets: HashMap::new(),
            clips: Vec::new(),
        };
        let mut wanted = node_ids.iter().map(String::as_str).collect::<HashSet<_>>();
        let mut active = Vec::<ActiveClip<'a>>::new();
        let mut stack = vec![Frame {
            nodes: page.children.iter(),
            ancestor_xform: glam::Affine2::IDENTITY,
            restore_clip_depth: 0,
        }];
        while !wanted.is_empty() {
            let Some(frame) = stack.last_mut() else {
                break;
            };
            let Some(node) = frame.nodes.next() else {
                let depth = frame.restore_clip_depth;
                stack.pop();
                active.truncate(depth);
                continue;
            };
            let ancestor_xform = frame.ancestor_xform;
            if wanted.remove(node.id.as_str()) {
                // Materialize only the union of requested ancestor prefixes.
                // Sibling targets share links instead of cloning whole chains.
                let mut parent = None;
                for clip in &mut active {
                    let at = if let Some(at) = clip.retained {
                        at
                    } else {
                        let at = batch.clips.len();
                        batch.clips.push(ClipLink {
                            clip: AncestorClip::for_node(clip.node, clip.xform),
                            parent,
                        });
                        clip.retained = Some(at);
                        at
                    };
                    parent = Some(at);
                }
                batch.targets.insert(
                    node.id.as_str(),
                    Target {
                        node,
                        ancestor_xform,
                        clip_head: parent,
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
            if node.clip_content && node.bounds.size.x != 0.0 && node.bounds.size.y != 0.0 {
                active.push(ActiveClip {
                    node,
                    xform: child_xform,
                    retained: None,
                });
            }
            stack.push(Frame {
                nodes: node.children.iter(),
                ancestor_xform: child_xform,
                restore_clip_depth: depth,
            });
        }
        batch
    }

    /// Serialize one requested ID using the unchanged node-context serializer.
    /// Calls may follow any input order, including repeated or missing IDs.
    /// Only this node's output and its small ancestor metadata are temporary.
    pub fn serialize(&self, node_id: &str, scale: f32) -> Result<String, SvgExportError> {
        let target = self
            .targets
            .get(node_id)
            .ok_or_else(|| SvgExportError::NodeNotFound(node_id.to_owned()))?;
        let mut head = target.clip_head;
        let mut clips = Vec::new();
        while let Some(index) = head {
            let link = &self.clips[index];
            clips.push(link.clip.clone());
            head = link.parent;
        }
        clips.reverse();
        serialize_node_context(target.node, node_id, target.ancestor_xform, &clips, scale)
    }
}
