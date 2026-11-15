//! Linear requested-selection geometry with first-preorder identity semantics.
use super::{own_bounds, union_of, DocRect};
use crate::{NodeId, PenNodeExt};
use jian_ops_schema::node::PenNode;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
struct Union(Option<(f64, f64, f64, f64)>);
impl Union {
    fn add(&mut self, bounds: DocRect) {
        if bounds.is_empty() {
            return;
        }
        let (x, y, r, b) = (bounds.x, bounds.y, bounds.x + bounds.w, bounds.y + bounds.h);
        self.0 = Some(match self.0 {
            None => (x, y, r, b),
            Some((left, top, right, bottom)) => {
                (left.min(x), top.min(y), right.max(r), bottom.max(b))
            }
        });
    }
    fn finish(self) -> DocRect {
        self.0.map_or(DocRect::ZERO, |(x, y, r, b)| DocRect {
            x,
            y,
            w: r - x,
            h: b - y,
        })
    }
}
struct Frame<'a> {
    children: std::slice::Iter<'a, PenNode>,
    id: Option<&'a str>,
    own: DocRect,
    capture: bool,
    union: Union,
}

pub(super) fn union(nodes: &[PenNode], ids: &[NodeId]) -> Option<DocRect> {
    let wanted: HashSet<&str> = ids.iter().map(NodeId::as_str).collect();
    let mut first = HashSet::with_capacity(wanted.len());
    let mut found: HashMap<&str, DocRect> = HashMap::with_capacity(wanted.len());
    let mut remaining = wanted.len();
    let mut stack = vec![Frame {
        children: nodes.iter(),
        id: None,
        own: DocRect::ZERO,
        capture: false,
        union: Union::default(),
    }];
    while let Some(parent) = stack.last_mut() {
        if let Some(node) = parent.children.next() {
            let own = own_bounds(node);
            let area = own.w > 0.0 || own.h > 0.0;
            // Claim the identity on entry, before any duplicate descendant.
            let capture = wanted.contains(node.id_str()) && first.insert(node.id_str());
            if capture && area {
                found.insert(node.id_str(), own);
                remaining -= 1;
                if remaining == 0 {
                    break;
                }
            }
            if let Some(children) = node.children().filter(|children| !children.is_empty()) {
                stack.push(Frame {
                    children: children.iter(),
                    id: Some(node.id_str()),
                    own,
                    capture: capture && !area,
                    union: Union::default(),
                });
            } else {
                let bounds = if area { own } else { DocRect::ZERO };
                if capture && !area {
                    found.insert(node.id_str(), bounds);
                    remaining -= 1;
                }
                parent.union.add(bounds);
                if remaining == 0 {
                    break;
                }
            }
        } else {
            let frame = stack.pop().expect("current frame exists");
            let bounds = if frame.own.w > 0.0 || frame.own.h > 0.0 {
                frame.own
            } else {
                frame.union.finish()
            };
            if frame.capture {
                found.insert(frame.id.expect("captured node has identity"), bounds);
                remaining -= 1;
            }
            if let Some(parent) = stack.last_mut() {
                parent.union.add(bounds);
            }
            if remaining == 0 {
                break;
            }
        }
    }
    // Preserve caller union order, duplicates, IEEE extrema arithmetic and
    // missing-ID filtering; only the lookup/derived-subtree work is shared.
    let result = union_of(ids.iter().filter_map(|id| found.get(id.as_str()).copied()));
    (!result.is_empty()).then_some(result)
}
