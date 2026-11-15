//! Sealed forward child-list state from a completed, exact scoped inverse.
use jian_ops_schema::node::PenNode;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanonicalScope<'a> {
    Root,
    Page { index: usize, id: &'a str },
}
/// The path is a validated CURRENT route, not permission to address an old
/// source byte span numerically. Logical scope/parent identity disambiguates it.
#[derive(Clone, Copy, Debug)]
pub struct CanonicalSpliceTarget<'a> {
    pub scope: CanonicalScope<'a>,
    pub parent_path: &'a [usize],
    pub parent_id: Option<&'a str>,
    pub parent_kind: Option<&'static str>,
}
/// Fields are private: consumers cannot construct/mutate a proof from an ID or
/// revision. Before order comes from the actual completed inverse, not a diff
/// inferred from a tool name. Caller must accept every companion delta or none.
pub struct CanonicalSplice<'a> {
    target: CanonicalSpliceTarget<'a>,
    before_present: bool,
    after_present: bool,
    before: Vec<&'a str>,
    after_len: usize,
    position: usize,
    removed_end: usize,
    inserted: &'a [PenNode],
    reusable: Vec<&'a str>,
}
impl<'a> CanonicalSplice<'a> {
    pub fn target(&self) -> CanonicalSpliceTarget<'a> {
        self.target
    }
    pub fn before_present(&self) -> bool {
        self.before_present
    }
    pub fn after_present(&self) -> bool {
        self.after_present
    }
    pub fn before_len(&self) -> usize {
        self.before.len()
    }
    pub fn after_len(&self) -> usize {
        self.after_len
    }
    pub fn position(&self) -> usize {
        self.position
    }
    pub fn left(&self) -> Option<&'a str> {
        self.position.checked_sub(1).map(|i| self.before[i])
    }
    pub fn right(&self) -> Option<&'a str> {
        self.before.get(self.removed_end).copied()
    }
    pub fn before_ids(&self) -> impl ExactSizeIterator<Item = &'a str> + Clone + '_ {
        self.before.iter().copied()
    }
    pub fn removed_ids(&self) -> impl ExactSizeIterator<Item = &'a str> + Clone + '_ {
        self.before[self.position..self.removed_end].iter().copied()
    }
    pub fn inserted(&self) -> &'a [PenNode] {
        self.inserted
    }
    /// Old bytes PLUS the other certified metadata/descendant deltas in this
    /// same all-or-nothing visit. Matching ID alone never grants this witness.
    /// Group members may be reusable nested descendants of a new wrapper.
    pub fn reusable_ids(&self) -> impl ExactSizeIterator<Item = &'a str> + Clone + '_ {
        self.reusable.iter().copied()
    }
    pub fn reuse_existing_bytes(&self, id: &str) -> bool {
        self.reusable.binary_search(&id).is_ok()
    }
    pub(crate) fn new(
        target: CanonicalSpliceTarget<'a>,
        before_present: bool,
        after_present: bool,
        before: Vec<&'a str>,
        after_len: usize,
        position: usize,
        removed_end: usize,
        inserted: &'a [PenNode],
        mut reusable: Vec<&'a str>,
    ) -> Self {
        reusable.sort_unstable();
        reusable.dedup();
        Self {
            target,
            before_present,
            after_present,
            before,
            after_len,
            position,
            removed_end,
            inserted,
            reusable,
        }
    }
    pub(crate) fn exclude_reuse(&mut self, excluded: &std::collections::HashSet<&str>) {
        self.reusable.retain(|id| !excluded.contains(id));
    }
    pub(crate) fn retained_bytes(&self) -> Option<usize> {
        self.before
            .capacity()
            .checked_add(self.reusable.capacity())?
            .checked_mul(std::mem::size_of::<&str>())
    }
}
