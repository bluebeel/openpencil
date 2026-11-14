//! Borrowed first-preorder export identities and shared ancestor/mask prefixes.

use crate::layout_scene::{NodeKind, SceneNode, ScenePage};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy)]
pub(super) enum Context<'a> {
    Ancestor(&'a SceneNode),
    Mask(&'a SceneNode),
}
struct Link<'a> {
    context: Context<'a>,
    parent: Option<usize>,
}
struct Active<'a> {
    context: Context<'a>,
    retained: Option<usize>,
}
struct Target<'a> {
    node: &'a SceneNode,
    head: Option<usize>,
    visible: bool,
}

/// Only immutable scene references and a depth-sized context view are owned.
/// A source mask is a required dependency, never an unrelated backdrop sibling.
pub struct SceneSelection<'a> {
    pub(super) node: &'a SceneNode,
    pub(super) contexts: Vec<Context<'a>>,
    pub(super) visible: bool,
}
impl<'a> SceneSelection<'a> {
    pub fn node(&self) -> &'a SceneNode {
        self.node
    }
    pub fn is_visible(&self) -> bool {
        self.visible
    }
}

struct Frame<'a> {
    nodes: &'a [SceneNode],
    at: usize,
    next_mask: Option<usize>,
    mask_installed: bool,
    base_depth: usize,
    restore_depth: usize,
    visible: bool,
    only_child: Option<&'a SceneNode>,
}
impl<'a> Frame<'a> {
    fn new(
        nodes: &'a [SceneNode],
        base: usize,
        restore: usize,
        visible: bool,
        only_child: Option<&'a SceneNode>,
    ) -> Self {
        let mut frame = Self {
            nodes,
            at: 0,
            next_mask: None,
            mask_installed: false,
            base_depth: base,
            restore_depth: restore,
            visible,
            only_child,
        };
        frame.next_mask = frame.find_mask(0);
        frame
    }
    fn paints_child(&self, node: &SceneNode) -> bool {
        self.only_child
            .is_none_or(|active| std::ptr::eq(active, node))
    }
    fn find_mask(&self, from: usize) -> Option<usize> {
        self.nodes[from..]
            .iter()
            .position(|node| {
                self.paints_child(node) && super::super::mask::mask_type(node).is_some()
            })
            .map(|offset| from + offset)
    }
}

/// One page walk for all requested IDs; preserves first-ID identity and
/// duplicates/order at the caller. Context prefixes share links across targets.
pub struct SceneSelectionBatch<'a> {
    targets: HashMap<&'a str, Target<'a>>,
    links: Vec<Link<'a>>,
}
impl<'a> SceneSelectionBatch<'a> {
    pub fn new<'id>(page: &'a ScenePage, node_ids: impl IntoIterator<Item = &'id str>) -> Self {
        let mut result = Self {
            targets: HashMap::new(),
            links: Vec::new(),
        };
        let mut wanted = node_ids.into_iter().collect::<HashSet<_>>();
        let mut active = Vec::<Active<'a>>::new();
        let mut stack = vec![Frame::new(&page.children, 0, 0, true, None)];
        while !wanted.is_empty() {
            let Some(frame) = stack.last_mut() else { break };
            let nodes = frame.nodes;
            let Some(node) = nodes.get(frame.at) else {
                let restore = frame.restore_depth;
                stack.pop();
                active.truncate(restore);
                continue;
            };
            let index = frame.at;
            frame.at += 1;
            let visible = frame.visible && frame.paints_child(node) && !node.hidden;
            if frame.next_mask == Some(index) {
                active.truncate(frame.base_depth);
                frame.next_mask = frame.find_mask(frame.at);
                frame.mask_installed = false;
            } else if !frame.mask_installed {
                if let Some(mask) = frame
                    .next_mask
                    .map(|index| &nodes[index])
                    .filter(|mask| !mask.hidden)
                {
                    active.push(Active {
                        context: Context::Mask(mask),
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
                        let at = result.links.len();
                        result.links.push(Link {
                            context: context.context,
                            parent,
                        });
                        context.retained = Some(at);
                        at
                    };
                    parent = Some(at);
                }
                result.targets.insert(
                    node.id.as_str(),
                    Target {
                        node,
                        head: parent,
                        visible,
                    },
                );
                if wanted.is_empty() {
                    break;
                }
            }
            if node.children.is_empty() {
                continue;
            }
            let depth = active.len();
            active.push(Active {
                context: Context::Ancestor(node),
                retained: None,
            });
            let recurses = matches!(
                node.kind,
                NodeKind::Frame | NodeKind::Group | NodeKind::Rect
            ) || matches!(&node.kind, NodeKind::Other(tag) if tag != "icon_font");
            let only_child = node
                .widget
                .as_ref()
                .filter(|widget| widget.kind == "tabs")
                .and_then(|_| node.visible_children().first());
            let no_active_tab = node
                .widget
                .as_ref()
                .is_some_and(|widget| widget.kind == "tabs")
                && only_child.is_none();
            stack.push(Frame::new(
                &node.children,
                active.len(),
                depth,
                visible && recurses && !no_active_tab,
                only_child,
            ));
        }
        result
    }
    pub fn get(&self, id: &str) -> Option<SceneSelection<'a>> {
        let target = self.targets.get(id)?;
        let mut contexts = Vec::new();
        let mut head = target.head;
        while let Some(index) = head {
            let link = &self.links[index];
            contexts.push(link.context);
            head = link.parent;
        }
        contexts.reverse();
        Some(SceneSelection {
            node: target.node,
            contexts,
            visible: target.visible,
        })
    }
}
