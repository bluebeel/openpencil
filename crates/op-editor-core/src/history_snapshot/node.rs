//! Schema-complete recursive sharing: canonical metadata plus immutable child edges.
use super::*;
use crate::pen_node_ext::clone_without_children;

#[derive(Debug, Clone, PartialEq)]
pub struct SharedNode {
    metadata: PenNode,
    sensitivity: Box<[u8]>,
    children: Option<Arc<Vec<Arc<SharedNode>>>>,
}
impl SharedNode {
    pub fn metadata(&self) -> &PenNode {
        &self.metadata
    }
    pub fn children(&self) -> Option<&[Arc<Self>]> {
        self.children.as_deref().map(Vec::as_slice)
    }
    /// Immutable child-edge owner, preserving absent versus present-empty.
    pub fn children_arc(&self) -> Option<&Arc<Vec<Arc<Self>>>> {
        self.children.as_ref()
    }
    pub fn id_str(&self) -> &str {
        self.metadata.id_str()
    }
    pub fn materialize(&self) -> PenNode {
        let mut node = self.metadata.clone();
        if let Some(children) = &self.children {
            let target = node.children_mut().expect("captured canonical container");
            *target = children.iter().map(|n| n.materialize()).collect();
        }
        node
    }
    /// Compare the complete source subtree without creating a hidden owned view.
    pub fn equals_node(&self, node: &PenNode) -> bool {
        let metadata = clone_without_children(node);
        if self.metadata != metadata || self.sensitivity != super::sensitivity::bytes(&metadata) {
            return false;
        }
        match (self.children(), node.children()) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.equals_node(b))
            }
            _ => false,
        }
    }
    pub fn find_node(&self, id: &NodeId) -> Option<&Self> {
        if self.id_str() == id.as_str() {
            return Some(self);
        }
        self.children()?.iter().find_map(|n| n.find_node(id))
    }
    /// Fixed node storage and sensitivity payload. Shared child-edge allocations
    /// are exposed separately so the resident ledger can deduplicate them.
    pub fn storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + 2 * std::mem::size_of::<usize>() + self.sensitivity.len()
    }
    pub fn child_edge_storage(&self) -> Option<(usize, usize)> {
        self.children.as_ref().map(super::edges::storage)
    }
    pub(crate) fn capture(node: &PenNode, anchor: Option<&Arc<Self>>) -> Arc<Self> {
        Capture::default().node(node, anchor)
    }
    pub(crate) fn repair(entry: &mut Arc<Self>, id: &NodeId, replacement: &PenNode) {
        let Some(found) = entry.find_node(id) else {
            return;
        };
        if matches!(found.metadata, PenNode::Ref(_)) {
            return;
        }
        if entry.id_str() == id.as_str() {
            *entry = Self::capture(replacement, None);
            return;
        }
        let index = entry
            .children()
            .and_then(|children| children.iter().position(|n| n.find_node(id).is_some()));
        if let Some(index) = index {
            let children = Arc::make_mut(entry).children.as_mut().unwrap();
            Self::repair(&mut Arc::make_mut(children)[index], id, replacement)
        }
    }
}

#[derive(Default)]
struct Capture {
    scratch: Vec<u8>,
}
impl Capture {
    fn node(&mut self, node: &PenNode, anchor: Option<&Arc<SharedNode>>) -> Arc<SharedNode> {
        // Unchanged leaves compare borrowed metadata without cloning strings,
        // paint arrays or text. Reuse the sensitivity buffer across siblings.
        if node.children().is_none_or(|c| c.is_empty()) {
            if let Some(old) = anchor {
                if old.children().is_none_or(|c| c.is_empty()) {
                    let same = match super::exact::geometry_matches(&old.metadata, node) {
                        Some(equal) => equal,
                        None => {
                            if old.metadata == *node {
                                super::sensitivity::write(node, &mut self.scratch);
                                old.sensitivity.as_ref() == self.scratch
                            } else {
                                false
                            }
                        }
                    };
                    if same {
                        return old.clone();
                    }
                }
            }
        }
        let metadata = clone_without_children(node);
        let children = node
            .children()
            .map(|nodes| self.children(nodes, anchor.and_then(|a| a.children.as_ref())));
        super::sensitivity::write(&metadata, &mut self.scratch);
        if let Some(old) = anchor {
            let same = match (&children, &old.children) {
                (None, None) => true,
                (Some(a), Some(b)) => {
                    a.len() == b.len() && a.iter().zip(b.iter()).all(|(a, b)| Arc::ptr_eq(a, b))
                }
                _ => false,
            };
            if same && old.metadata == metadata && old.sensitivity.as_ref() == self.scratch {
                return old.clone();
            }
        }
        Arc::new(SharedNode {
            metadata,
            children,
            sensitivity: self.scratch.clone().into_boxed_slice(),
        })
    }
    fn children(
        &mut self,
        current: &[PenNode],
        prev: Option<&Arc<Vec<Arc<SharedNode>>>>,
    ) -> Arc<Vec<Arc<SharedNode>>> {
        let previous = prev.map(|v| v.as_slice()).unwrap_or(&[]);
        let mut by_id = None;
        let captured = current
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let anchor = previous
                    .get(index)
                    .filter(|n| n.id_str() == node.id_str())
                    .or_else(|| {
                        by_id
                            .get_or_insert_with(|| {
                                previous
                                    .iter()
                                    .map(|n| (n.id_str(), n))
                                    .collect::<HashMap<_, _>>()
                            })
                            .get(node.id_str())
                            .copied()
                    });
                self.node(node, anchor)
            })
            .collect();
        super::edges::reuse(captured, prev)
    }
}
pub(super) fn share_children(
    current: &[PenNode],
    prev: Option<&Arc<Vec<Arc<SharedNode>>>>,
) -> Arc<Vec<Arc<SharedNode>>> {
    Capture::default().children(current, prev)
}
impl Serialize for SharedNode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.metadata
            .serialize(super::node_wire::ReplaceChildren::new(
                serializer,
                self.children(),
            ))
    }
}
