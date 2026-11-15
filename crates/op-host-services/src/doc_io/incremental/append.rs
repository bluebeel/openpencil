//! Bounded suffix overlays against immutable writer-owned child-array spans.
//! Appended IDs deliberately have no metadata span: their later edits reseed
//! through the full writer instead of guessing new output offsets.
use super::{budget, index::Children, splice, Base, Span};
use jian_ops_schema::node::PenNode;
use op_editor_core::{CanonicalAppend, CanonicalChildTarget, PenNodeExt};
use std::{
    collections::{BTreeMap, HashSet},
    mem,
    ops::Bound::{Excluded, Unbounded},
};
const MAX_IDS: usize = 8192;
const MAX_NEW_NODES: usize = 4096;
const EDIT_LIMIT: usize = budget::EDITS - 1024 * 1024;
#[derive(Default)]
pub(super) struct Overlay {
    lists: BTreeMap<usize, State>,
    ids: HashSet<String>,
    strings: usize,
}
struct State {
    len: usize,
    last: String,
}
impl Overlay {
    pub(super) fn bytes(&self) -> usize {
        self.strings
            .saturating_add(
                self.ids
                    .capacity()
                    .saturating_mul(mem::size_of::<String>() + 16),
            )
            .saturating_add(
                self.lists
                    .len()
                    .saturating_mul(mem::size_of::<(usize, State)>() + 1024),
            )
    }
}
impl Base {
    fn child_array(&self, target: CanonicalChildTarget<'_>) -> Option<Children> {
        match target {
            CanonicalChildTarget::Root => self.index.root,
            CanonicalChildTarget::Page { id, .. } => self.index.pages.get(id).copied().flatten(),
            CanonicalChildTarget::Node { id } => {
                self.index.nodes.get(id).copied().flatten()?.children
            }
        }
    }
    fn before_append(&self, list: Children, len: usize, last: Option<&str>) -> Option<()> {
        if let Some(current) = self.appends.lists.get(&list.span.start) {
            return (current.len == len && Some(current.last.as_str()) == last).then_some(());
        }
        if list.len != len {
            return None;
        }
        match (last, list.last) {
            (None, None) if len == 0 => Some(()),
            (Some(id), Some(at)) => self
                .index
                .nodes
                .get(id)
                .copied()
                .flatten()
                .filter(|node| node.object.start == at)
                .map(|_| ()),
            _ => None,
        }
    }
    fn new_ids<'a>(&self, nodes: &'a [PenNode]) -> Option<Vec<&'a str>> {
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        let mut pending = vec![nodes.iter()];
        while let Some(siblings) = pending.last_mut() {
            let Some(node) = siblings.next() else {
                pending.pop();
                continue;
            };
            if result.len() == MAX_NEW_NODES
                || node.id_str().is_empty()
                || node.id_str().len() > 16 * 1024
                || self.index.nodes.contains_key(node.id_str())
                || self.appends.ids.contains(node.id_str())
                || !seen.insert(node.id_str())
                || matches!(node, PenNode::Ref(_))
                || matches!(node,PenNode::Frame(frame)if frame.reusable==Some(true))
            {
                return None;
            }
            result.push(node.id_str());
            if let Some(children) = node.children().filter(|children| !children.is_empty()) {
                if pending.len() == 128 {
                    return None;
                }
                pending.push(children.iter());
            }
        }
        (!result.is_empty() && self.appends.ids.len().checked_add(result.len())? <= MAX_IDS)
            .then_some(result)
    }
    pub(super) fn append(&mut self, receipt: CanonicalAppend<'_>) -> Option<()> {
        if receipt.nodes.is_empty() {
            return None;
        }
        let list = self.child_array(receipt.target)?;
        self.before_append(list, receipt.previous_len, receipt.previous_last)?;
        let ids = self.new_ids(receipt.nodes)?;
        let encoded = splice::append(receipt.nodes)?;
        let payload = encoded.strip_prefix(b"[")?.strip_suffix(b"]")?;
        if payload.is_empty() {
            return None;
        }
        let len = receipt.previous_len.checked_add(receipt.nodes.len())?;
        let last = receipt.nodes.last()?.id_str();
        self.appends.ids.try_reserve(ids.len()).ok()?;
        for id in ids {
            let owned = id.to_owned();
            self.appends.strings = self.appends.strings.checked_add(owned.capacity())?;
            self.appends.ids.insert(owned);
            if self.appends.bytes().checked_add(self.replacement_bytes)? > EDIT_LIMIT {
                return None;
            }
        }
        let last = last.to_owned();
        let old = self
            .appends
            .lists
            .insert(list.span.start, State { len, last });
        self.appends.strings = self
            .appends
            .strings
            .checked_sub(old.as_ref().map_or(0, |state| state.last.capacity()))?
            .checked_add(self.appends.lists.get(&list.span.start)?.last.capacity())?;
        if self.appends.bytes().checked_add(self.replacement_bytes)? > EDIT_LIMIT {
            return None;
        }
        let point = Span {
            start: list.span.end.checked_sub(1)?,
            end: list.span.end - 1,
        };
        self.append_fragment(point, receipt.previous_len != 0, payload)
    }
    fn append_fragment(&mut self, span: Span, comma: bool, payload: &[u8]) -> Option<()> {
        if !self.replacements.contains_key(&span) && self.replacements.len() >= 8192 {
            return None;
        }
        for (other, _) in self
            .replacements
            .range(..span)
            .next_back()
            .into_iter()
            .chain(self.replacements.range((Excluded(span), Unbounded)).next())
        {
            if span.overlaps(*other) {
                return None;
            }
        }
        let old = self.replacements.get(&span).map_or(0, Vec::capacity);
        let rest = self
            .replacement_bytes
            .checked_sub(old)?
            .checked_add(self.appends.bytes())?;
        let available = EDIT_LIMIT.checked_sub(rest)?;
        let bytes = self.replacements.entry(span).or_default();
        let required = bytes
            .len()
            .checked_add(usize::from(comma))?
            .checked_add(payload.len())?;
        if required > available {
            return None;
        }
        if required > bytes.capacity() {
            let target = required
                .max(bytes.capacity().saturating_mul(2))
                .min(available);
            bytes
                .try_reserve_exact(target.checked_sub(bytes.len())?)
                .ok()?;
        }
        if bytes.capacity() > available {
            return None;
        }
        self.replacement_bytes = self
            .replacement_bytes
            .checked_sub(old)?
            .checked_add(bytes.capacity())?;
        if comma {
            bytes.push(b',');
        }
        bytes.extend_from_slice(payload);
        Some(())
    }
}
