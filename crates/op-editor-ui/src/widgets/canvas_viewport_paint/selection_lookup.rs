//! Borrowed first-preorder export identities and shared ancestor/mask prefixes.

use crate::layout_scene::{NodeKind, SceneNode, ScenePage};
use crate::Rect;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

#[derive(Clone)]
pub(super) enum Context<'a> {
    Ancestor(&'a SceneNode),
    Mask(&'a SceneNode, Arc<OnceLock<MaskBounds>>),
}
#[derive(Clone, Copy)]
pub(super) enum MaskBounds {
    Unsupported,
    Supported(Option<Rect>),
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
    bounds: Arc<OnceLock<Option<Rect>>>,
}

/// Only immutable scene references and a depth-sized context view are owned.
/// A source mask is a required dependency, never an unrelated backdrop sibling.
pub struct SceneSelection<'a> {
    pub(super) node: &'a SceneNode,
    pub(super) contexts: Vec<Context<'a>>,
    pub(super) visible: bool,
    pub(super) bounds: Arc<OnceLock<Option<Rect>>>,
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
                        context: Context::Mask(mask, Arc::new(OnceLock::new())),
                        retained: None,
                    });
                }
                frame.mask_installed = true;
            }
            if wanted.remove(node.id.as_str()) {
                // A retained active prefix is immutable until popped. Start
                // at its last link rather than rescan every ancestor for each
                // sibling target; each new prefix suffix is materialized once.
                let retained = active
                    .iter()
                    .rposition(|context| context.retained.is_some());
                let mut parent = retained.and_then(|at| active[at].retained);
                for context in &mut active[retained.map_or(0, |at| at + 1)..] {
                    let at = if let Some(at) = context.retained {
                        at
                    } else {
                        let at = result.links.len();
                        result.links.push(Link {
                            context: context.context.clone(),
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
                        bounds: Arc::new(OnceLock::new()),
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
            contexts.push(link.context.clone());
            head = link.parent;
        }
        contexts.reverse();
        Some(SceneSelection {
            node: target.node,
            contexts,
            visible: target.visible,
            bounds: Arc::clone(&target.bounds),
        })
    }
    /// Reuse one depth-sized context workspace across ordered exports. The
    /// legacy get() remains detached from the lifetime of this batch.
    pub fn cursor(&self) -> SceneSelectionCursor<'a> {
        SceneSelectionCursor { selection: None }
    }
}

/// A single consumer's reusable branch view. Missing IDs leave the allocation
/// available for the next target; no scene or output payload is cloned.
pub struct SceneSelectionCursor<'a> {
    selection: Option<SceneSelection<'a>>,
}
impl<'a> SceneSelectionCursor<'a> {
    pub fn get<'view>(
        &'view mut self,
        batch: &SceneSelectionBatch<'a>,
        id: &str,
    ) -> Option<&'view SceneSelection<'a>> {
        let target = batch.targets.get(id)?;
        let initialized = self.selection.is_some();
        let selection = self.selection.get_or_insert_with(|| SceneSelection {
            node: target.node,
            contexts: Vec::new(),
            visible: target.visible,
            bounds: Arc::clone(&target.bounds),
        });
        // Bounds are allocated privately once per immutable batch target.
        // Their live Arc identity proves this is that exact target/context,
        // including across repeated IDs; no address/revision-only cache key.
        if initialized && Arc::ptr_eq(&selection.bounds, &target.bounds) {
            return Some(selection);
        }
        selection.node = target.node;
        selection.visible = target.visible;
        selection.bounds = Arc::clone(&target.bounds);
        selection.contexts.clear();
        let mut head = target.head;
        while let Some(index) = head {
            let link = &batch.links[index];
            selection.contexts.push(link.context.clone());
            head = link.parent;
        }
        selection.contexts.reverse();
        Some(selection)
    }
}
