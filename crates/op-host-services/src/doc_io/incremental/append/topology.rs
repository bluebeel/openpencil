//! Exact array plans from sealed completed inverses; never a shadow old forest.
use super::*;
use op_editor_core::{CanonicalScope, CanonicalSplice, CanonicalSpliceTarget};
const MAX_ARRAY_IDS: usize = 8192;
const MAX_ID_BYTES: usize = 512 * 1024;
struct Prepared<'p, 'a> {
    receipt: &'p CanonicalSplice<'a>,
    array: Array,
    before: Vec<&'a str>,
}
fn key(array: Array) -> Location {
    Location {
        source: array.source,
        at: array.list.span.start,
    }
}
fn array_conflict(span: Span, array: Span) -> bool {
    span.overlaps(array) && !(array.start <= span.start && span.end <= array.end)
}
impl Overlay {
    pub(super) fn array_overlap(&self, source: Source, span: Span) -> bool {
        self.arrays
            .iter()
            .any(|(location, plan)| location.source == source && array_conflict(span, plan.span))
    }
}
impl Base {
    fn splice_array(&self, target: CanonicalSpliceTarget<'_>) -> Option<Array> {
        if let CanonicalScope::Page { id, .. } = target.scope {
            self.index.pages.get(id).copied().flatten()?;
        }
        match target.parent_id {
            Some(id) if !target.parent_path.is_empty() && target.parent_kind != Some("ref") => {
                let node = self.placed_node(id)?;
                Some(Array {
                    source: node.source,
                    list: node.node.children?,
                })
            }
            None if target.parent_path.is_empty() => match target.scope {
                CanonicalScope::Root => Some(Array {
                    source: Source::Raw,
                    list: self.index.root?,
                }),
                CanonicalScope::Page { id, .. } => Some(Array {
                    source: Source::Raw,
                    list: self.index.pages.get(id).copied().flatten()?,
                }),
            },
            _ => None,
        }
    }
    pub(in crate::doc_io::incremental) fn apply_splices<'a>(
        &mut self,
        receipts: &[CanonicalSplice<'a>],
    ) -> Option<()> {
        let mut prepared = Vec::new();
        let mut destinations = HashSet::new();
        let mut removed = HashSet::new();
        let mut reusable = HashSet::new();
        let mut id_bytes = 0usize;
        // Validate every source/destination before any same-ID remapping.
        for receipt in receipts {
            if !receipt.before_present()
                || !receipt.after_present()
                || receipt.before_len() > MAX_ARRAY_IDS
                || receipt.after_len() > MAX_ARRAY_IDS
            {
                return None;
            }
            let array = self.splice_array(receipt.target())?;
            if !destinations.insert(key(array)) {
                return None;
            }
            let before = receipt.before_ids().collect::<Vec<_>>();
            if before.len() != receipt.before_len() {
                return None;
            }
            let mut unique = HashSet::new();
            for id in &before {
                id_bytes = id_bytes.checked_add(id.len())?;
                if id_bytes > MAX_ID_BYTES || !unique.insert(*id) || self.placed_node(id).is_none()
                {
                    return None;
                }
            }
            if let Some(plan) = self.appends.arrays.get(&key(array)) {
                if plan
                    .ids
                    .iter()
                    .map(String::as_str)
                    .ne(before.iter().copied())
                {
                    return None;
                }
            } else {
                self.before_append(array, before.len(), before.last().copied())?;
            }
            let gone = receipt.removed_ids().collect::<Vec<_>>();
            let end = receipt.position().checked_add(gone.len())?;
            if before.get(receipt.position()..end)? != gone.as_slice()
                || before
                    .len()
                    .checked_sub(gone.len())?
                    .checked_add(receipt.inserted().len())?
                    != receipt.after_len()
                || receipt
                    .position()
                    .checked_sub(1)
                    .and_then(|i| before.get(i))
                    .copied()
                    != receipt.left()
                || before.get(end).copied() != receipt.right()
            {
                return None;
            }
            for id in gone {
                if self.placed_node(id)?.node.flags != 0 {
                    return None;
                }
                removed.insert(id);
            }
            reusable.extend(receipt.reusable_ids());
            prepared.push(Prepared {
                receipt,
                array,
                before,
            });
        }
        let mut produced = HashSet::new();
        let mut final_orders = Vec::new();
        for plan in prepared {
            let receipt = plan.receipt;
            let end = receipt.position() + receipt.removed_ids().len();
            let mut after = Vec::with_capacity(receipt.after_len());
            after.extend(
                plan.before[..receipt.position()]
                    .iter()
                    .map(|id| (*id).to_owned()),
            );
            for node in receipt.inserted() {
                after.push(self.canonical_piece(node, &reusable, &removed, &mut produced, 0)?);
            }
            after.extend(plan.before[end..].iter().map(|id| (*id).to_owned()));
            if after.len() != receipt.after_len() {
                return None;
            }
            final_orders.push((plan.array, after));
        }
        let mut final_ids = HashSet::new();
        if final_orders
            .iter()
            .any(|(_, ids)| ids.iter().any(|id| !final_ids.insert(id.as_str())))
        {
            return None;
        }
        for (array, ids) in final_orders {
            self.appends.arrays.insert(
                key(array),
                ArrayPlan {
                    span: array.list.span,
                    ids,
                },
            );
        }
        self.within_edit_budget()
    }
    fn within_edit_budget(&self) -> Option<()> {
        (self.appends.bytes().checked_add(self.replacement_bytes)? <= EDIT_LIMIT).then_some(())
    }
    fn canonical_piece(
        &mut self,
        node: &PenNode,
        reusable: &HashSet<&str>,
        removed: &HashSet<&str>,
        produced: &mut HashSet<String>,
        depth: usize,
    ) -> Option<String> {
        if depth >= 128
            || !eligible(node)
            || node.id_str().is_empty()
            || node.id_str().len() > 16 * 1024
        {
            return None;
        }
        let id = node.id_str();
        if self.index.nodes.get(id).is_some_and(Option::is_none) {
            return None;
        }
        if !produced.insert(id.to_owned()) {
            return None;
        }
        if reusable.contains(id) {
            return (self.placed_node(id)?.node.flags == 0).then(|| id.to_owned());
        }
        if self.placed_node(id).is_some() && !removed.contains(id) {
            return None;
        }
        if self.appends.nodes.len() >= MAX_IDS || self.appends.segments.len() >= MAX_IDS {
            return None;
        }
        let header = splice::metadata(node)?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(header.len().checked_add(2)?).ok()?;
        bytes.push(b'[');
        bytes.extend_from_slice(&header);
        bytes.push(b']');
        let mut indexed = index::appended_nodes(&bytes)?;
        if indexed.len() != 1 || indexed[0].0 != id {
            return None;
        }
        let (name, record) = indexed.pop()?;
        let segment = self.appends.segments.len();
        self.appends.segments.try_reserve(1).ok()?;
        self.appends.nodes.try_reserve(1).ok()?;
        self.appends.segments.push(Segment {
            bytes,
            patches: BTreeMap::new(),
            points: BTreeMap::new(),
            depth: depth + 1,
        });
        self.appends.nodes.insert(
            name,
            PlacedNode {
                source: Source::Segment(segment),
                node: record,
            },
        );
        if let Some(children) = node.children() {
            if children.len() > MAX_ARRAY_IDS {
                return None;
            }
            let array = Array {
                source: Source::Segment(segment),
                list: record.children?,
            };
            let mut ids = Vec::with_capacity(children.len());
            for child in children {
                ids.push(self.canonical_piece(child, reusable, removed, produced, depth + 1)?);
            }
            self.appends.arrays.insert(
                key(array),
                ArrayPlan {
                    span: array.list.span,
                    ids,
                },
            );
        }
        self.within_edit_budget()?;
        Some(id.to_owned())
    }
}
