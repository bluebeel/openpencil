//! Immutable writer-produced suffix segments. Every new-node span stays local
//! to its original bytes, so later edits never shift/reindex another fragment.
use super::{
    budget,
    index::{self, Children, Node},
    splice, Base, Span,
};
use jian_ops_schema::node::PenNode;
use op_editor_core::{CanonicalAppend, CanonicalChildTarget, PenNodeExt};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    mem,
};
mod metadata;
mod topology;
mod write;
const MAX_IDS: usize = 8192;
const MAX_NEW_NODES: usize = 4096;
const MAX_SEGMENT_DEPTH: usize = 128;
const EDIT_LIMIT: usize = budget::EDITS - 1024 * 1024;
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Source {
    Raw,
    Segment(usize),
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Location {
    source: Source,
    at: usize,
}
#[derive(Clone, Copy)]
struct PlacedNode {
    source: Source,
    node: Node,
}
#[derive(Clone, Copy)]
struct Link {
    segment: usize,
    comma: bool,
}
struct ArrayPlan {
    span: Span,
    ids: Vec<String>,
}
struct Segment {
    bytes: Vec<u8>,
    patches: BTreeMap<Span, Vec<u8>>,
    points: BTreeMap<usize, Vec<Link>>,
    depth: usize,
}
#[derive(Default)]
pub(super) struct Overlay {
    lists: BTreeMap<Location, State>,
    arrays: BTreeMap<Location, ArrayPlan>,
    nodes: HashMap<String, PlacedNode>,
    segments: Vec<Segment>,
    raw_points: BTreeMap<usize, Vec<Link>>,
}
struct State {
    len: usize,
    last: String,
}
#[derive(Clone, Copy)]
struct Array {
    source: Source,
    list: Children,
}
fn points_bytes(points: &BTreeMap<usize, Vec<Link>>) -> usize {
    points
        .len()
        .saturating_mul(1024 + mem::size_of::<(usize, Vec<Link>)>())
        .saturating_add(
            points
                .values()
                .map(|links| links.capacity() * mem::size_of::<Link>())
                .sum::<usize>(),
        )
}
impl Overlay {
    pub(super) fn bytes(&self) -> usize {
        let mut bytes = self
            .nodes
            .capacity()
            .saturating_mul(mem::size_of::<(String, PlacedNode)>() + 16)
            .saturating_add(self.nodes.keys().map(String::capacity).sum::<usize>())
            .saturating_add(
                self.lists
                    .len()
                    .saturating_mul(mem::size_of::<(Location, State)>() + 1024),
            )
            .saturating_add(
                self.lists
                    .values()
                    .map(|state| state.last.capacity())
                    .sum::<usize>(),
            )
            .saturating_add(
                self.segments
                    .capacity()
                    .saturating_mul(mem::size_of::<Segment>()),
            )
            .saturating_add(points_bytes(&self.raw_points));
        bytes = bytes
            .saturating_add(
                self.arrays
                    .len()
                    .saturating_mul(mem::size_of::<(Location, ArrayPlan)>() + 1024),
            )
            .saturating_add(
                self.arrays
                    .values()
                    .map(|plan| {
                        plan.ids.capacity() * mem::size_of::<String>()
                            + plan.ids.iter().map(String::capacity).sum::<usize>()
                    })
                    .sum::<usize>(),
            );
        for segment in &self.segments {
            bytes = bytes
                .saturating_add(segment.bytes.capacity())
                .saturating_add(points_bytes(&segment.points))
                .saturating_add(
                    segment
                        .patches
                        .len()
                        .saturating_mul(mem::size_of::<(Span, Vec<u8>)>() + 1024),
                )
                .saturating_add(segment.patches.values().map(Vec::capacity).sum::<usize>());
        }
        bytes
    }
    fn points_mut(&mut self, source: Source) -> Option<&mut BTreeMap<usize, Vec<Link>>> {
        match source {
            Source::Raw => Some(&mut self.raw_points),
            Source::Segment(id) => Some(&mut self.segments.get_mut(id)?.points),
        }
    }
    pub(super) fn raw_overlap(&self, span: Span) -> bool {
        self.array_overlap(Source::Raw, span)
            || self
                .raw_points
                .range(span.start..span.end)
                .any(|(point, _)| span.start < *point)
    }
}
fn overlap(
    patches: &BTreeMap<Span, Vec<u8>>,
    points: &BTreeMap<usize, Vec<Link>>,
    span: Span,
) -> bool {
    // All installed replacements are disjoint; only immediate neighbors can
    // contain/overlap a new range. Insertion points strictly inside it refuse.
    use std::ops::Bound::{Excluded, Unbounded};
    patches
        .range(..span)
        .next_back()
        .into_iter()
        .chain(patches.range((Excluded(span), Unbounded)).next())
        .any(|(other, _)| span.overlaps(*other))
        || points
            .range(span.start..span.end)
            .any(|(point, _)| span.start < *point)
}
impl Base {
    fn placed_node(&self, id: &str) -> Option<PlacedNode> {
        if let Some(node) = self.appends.nodes.get(id) {
            return Some(*node);
        }
        match self.index.nodes.get(id) {
            Some(node) => node.map(|node| PlacedNode {
                source: Source::Raw,
                node,
            }),
            None => self.appends.nodes.get(id).copied(),
        }
    }
    fn child_array(&self, target: CanonicalChildTarget<'_>) -> Option<Array> {
        match target {
            CanonicalChildTarget::Root => Some(Array {
                source: Source::Raw,
                list: self.index.root?,
            }),
            CanonicalChildTarget::Page { id, .. } => Some(Array {
                source: Source::Raw,
                list: self.index.pages.get(id).copied().flatten()?,
            }),
            CanonicalChildTarget::Node { id } => {
                let node = self.placed_node(id)?;
                Some(Array {
                    source: node.source,
                    list: node.node.children?,
                })
            }
        }
    }
    fn before_append(&self, array: Array, len: usize, last: Option<&str>) -> Option<()> {
        let key = Location {
            source: array.source,
            at: array.list.span.start,
        };
        if let Some(plan) = self.appends.arrays.get(&key) {
            return (plan.ids.len() == len && plan.ids.last().map(String::as_str) == last)
                .then_some(());
        }
        if let Some(current) = self.appends.lists.get(&key) {
            return (current.len == len && Some(current.last.as_str()) == last).then_some(());
        }
        if array.list.len != len {
            return None;
        }
        match (last, array.list.last) {
            (None, None) if len == 0 => Some(()),
            (Some(id), Some(at)) => self
                .placed_node(id)
                .filter(|node| node.source == array.source && node.node.object.start == at)
                .map(|_| ()),
            _ => None,
        }
    }
    fn new_ids<'a>(&self, nodes: &'a [PenNode]) -> Option<HashSet<&'a str>> {
        let mut result = HashSet::new();
        let mut pending = vec![nodes.iter()];
        while let Some(siblings) = pending.last_mut() {
            let Some(node) = siblings.next() else {
                pending.pop();
                continue;
            };
            if result.len() == MAX_NEW_NODES
                || !eligible(node)
                || node.id_str().is_empty()
                || node.id_str().len() > 16 * 1024
                || self.index.nodes.contains_key(node.id_str())
                || self.appends.nodes.contains_key(node.id_str())
                || !result.insert(node.id_str())
            {
                return None;
            }
            if let Some(children) = node.children().filter(|children| !children.is_empty()) {
                if pending.len() == 128 {
                    return None;
                }
                pending.push(children.iter());
            }
        }
        (!result.is_empty() && self.appends.nodes.len().checked_add(result.len())? <= MAX_IDS)
            .then_some(result)
    }
    pub(super) fn append(&mut self, receipt: CanonicalAppend<'_>) -> Option<()> {
        let array = self.child_array(receipt.target)?;
        if self.appends.arrays.contains_key(&Location {
            source: array.source,
            at: array.list.span.start,
        }) {
            return None;
        }
        self.before_append(array, receipt.previous_len, receipt.previous_last)?;
        let ids = self.new_ids(receipt.nodes)?;
        let encoded = splice::append(receipt.nodes)?;
        let indexed = index::appended_nodes(&encoded)?;
        if indexed.len() != ids.len() {
            return None;
        }
        let mut encoded_ids = HashSet::new();
        if indexed
            .iter()
            .any(|(id, _)| !ids.contains(id.as_str()) || !encoded_ids.insert(id.as_str()))
        {
            return None;
        }
        drop(encoded_ids);
        let depth = match array.source {
            Source::Raw => 1,
            Source::Segment(id) => self.appends.segments.get(id)?.depth.checked_add(1)?,
        };
        if depth > MAX_SEGMENT_DEPTH || self.appends.segments.len() == MAX_IDS {
            return None;
        }
        let point = array.list.span.end.checked_sub(1)?;
        let point_span = Span {
            start: point,
            end: point,
        };
        match array.source {
            Source::Raw => {
                if overlap(&self.replacements, &self.appends.raw_points, point_span) {
                    return None;
                }
            }
            Source::Segment(id) => {
                let segment = self.appends.segments.get(id)?;
                if overlap(&segment.patches, &segment.points, point_span) {
                    return None;
                }
            }
        }
        self.appends.nodes.try_reserve(indexed.len()).ok()?;
        self.appends.segments.try_reserve(1).ok()?;
        let segment = self.appends.segments.len();
        self.appends.segments.push(Segment {
            bytes: encoded,
            patches: BTreeMap::new(),
            points: BTreeMap::new(),
            depth,
        });
        for (id, node) in indexed {
            self.appends.nodes.insert(
                id,
                PlacedNode {
                    source: Source::Segment(segment),
                    node,
                },
            );
        }
        let links = self
            .appends
            .points_mut(array.source)?
            .entry(point)
            .or_default();
        links.try_reserve(1).ok()?;
        links.push(Link {
            segment,
            comma: receipt.previous_len != 0,
        });
        self.appends.lists.insert(
            Location {
                source: array.source,
                at: array.list.span.start,
            },
            State {
                len: receipt.previous_len.checked_add(receipt.nodes.len())?,
                last: receipt.nodes.last()?.id_str().to_owned(),
            },
        );
        (self.appends.bytes().checked_add(self.replacement_bytes)? <= EDIT_LIMIT).then_some(())
    }
}
pub(super) fn eligible(node: &PenNode) -> bool {
    !matches!(node, PenNode::Ref(_))
        && !matches!(node,PenNode::Frame(frame)if frame.reusable==Some(true))
}
