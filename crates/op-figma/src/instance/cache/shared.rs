//! Borrowed exact-key lookup and one immutable expansion allocation. The
//! original mutable-owned wrapper is retained for compatibility controls.
use super::*;
const ENTRY_ALLOWANCE: usize = 256;

#[derive(Clone, Copy)]
pub(super) struct KeyView<'a> {
    component: &'a str,
    overrides: &'a [FigValue],
    derived: &'a [FigValue],
    size: Option<ExactSize>,
    global: u64,
    epoch: u64,
}
impl KeyView<'_> {
    pub(super) fn from_owned(key: &InstanceExpansionKey) -> KeyView<'_> {
        let InstanceExpansionKey {
            component_guid,
            overrides,
            derived,
            size,
            global_assignment_epoch,
            assignment_epoch,
        } = key;
        KeyView {
            component: component_guid,
            overrides,
            derived,
            size: *size,
            global: *global_assignment_epoch,
            epoch: *assignment_epoch,
        }
    }
    fn matches(&self, key: &InstanceExpansionKey) -> bool {
        self.component == key.component_guid
            && self.size == key.size
            && self.global == key.global_assignment_epoch
            && self.epoch == key.assignment_epoch
            && fig_values_equal(self.overrides, &key.overrides)
            && fig_values_equal(self.derived, &key.derived)
    }
    fn digest(&self) -> u64 {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        self.component.hash(&mut hash);
        self.global.hash(&mut hash);
        self.epoch.hash(&mut hash);
        self.size.hash(&mut hash);
        hash_fig_values(self.overrides, &mut hash);
        hash_fig_values(self.derived, &mut hash);
        hash.finish()
    }
    fn copied_key_bound(&self) -> usize {
        std::mem::size_of::<InstanceExpansionKey>()
            + 2 * std::mem::size_of::<usize>()
            + self.component.len()
            + self.overrides.iter().map(fig_value_bytes).sum::<usize>()
            + self.derived.iter().map(fig_value_bytes).sum::<usize>()
    }
    fn owned(&self) -> InstanceExpansionKey {
        InstanceExpansionKey {
            component_guid: self.component.into(),
            overrides: self.overrides.to_vec(),
            derived: self.derived.to_vec(),
            size: self.size,
            global_assignment_epoch: self.global,
            assignment_epoch: self.epoch,
        }
    }
}
fn shared_children_bytes(children: &Vec<TreeNode>) -> usize {
    std::mem::size_of::<Vec<TreeNode>>()
        + 2 * std::mem::size_of::<usize>()
        + (children.capacity() - children.len()) * std::mem::size_of::<TreeNode>()
        + children.iter().map(tree_structural_bytes).sum::<usize>()
}
impl InstanceExpansionCache {
    fn get_shared(&mut self, view: KeyView<'_>) -> Option<Rc<Vec<TreeNode>>> {
        let found = self
            .entries
            .get(&view.digest())
            .and_then(|bucket| bucket.iter().find(|entry| view.matches(&entry.key)))
            .map(|entry| Rc::clone(&entry.children));
        if found.is_some() {
            self.hits += 1;
        } else {
            self.misses += 1;
        }
        found
    }
    fn insert_shared(&mut self, view: KeyView<'_>, children: Rc<Vec<TreeNode>>) {
        if self.max_entries == 0 || self.max_structural_bytes == 0 {
            return;
        }
        let bound = ENTRY_ALLOWANCE + view.copied_key_bound() + shared_children_bytes(&children);
        // Reject a one-off/oversized expansion before cloning any key payload.
        if bound > self.max_structural_bytes {
            return;
        }
        self.insert_owned(view.owned(), children);
    }
    fn insert_owned(&mut self, key: InstanceExpansionKey, children: Rc<Vec<TreeNode>>) {
        if self.max_entries == 0 || self.max_structural_bytes == 0 {
            return;
        }
        let view = KeyView::from_owned(&key);
        let hash = view.digest();
        let structural_bytes =
            ENTRY_ALLOWANCE + key.structural_bytes() + shared_children_bytes(&children);
        if structural_bytes > self.max_structural_bytes
            || self
                .entries
                .get(&hash)
                .is_some_and(|bucket| bucket.iter().any(|entry| view.matches(&entry.key)))
        {
            return;
        }
        while !self.insertion_order.is_empty()
            && (self.insertion_order.len() >= self.max_entries
                || self.structural_bytes.saturating_add(structural_bytes)
                    > self.max_structural_bytes)
        {
            let Some((old_hash, oldest)) = self.insertion_order.pop_front() else {
                break;
            };
            let empty = if let Some(bucket) = self.entries.get_mut(&old_hash) {
                if let Some(at) = bucket
                    .iter()
                    .position(|entry| Rc::ptr_eq(&entry.key, &oldest))
                {
                    self.structural_bytes -= bucket.remove(at).structural_bytes;
                }
                bucket.is_empty()
            } else {
                false
            };
            if empty {
                self.entries.remove(&old_hash);
            }
        }
        let key = Rc::new(key);
        self.insertion_order.push_back((hash, Rc::clone(&key)));
        self.structural_bytes = self.structural_bytes.saturating_add(structural_bytes);
        self.entries.entry(hash).or_default().push(CachedExpansion {
            key,
            children,
            structural_bytes,
        });
    }
}

pub(crate) fn apply_instance_overrides_shared(
    cache: &mut InstanceExpansionCache,
    component_guid: &str,
    symbol_node: &TreeNode,
    overrides: Option<&[FigValue]>,
    derived: Option<&[FigValue]>,
    instance_size: Option<FigVec2>,
    assignment_cache: &mut HashMap<String, String>,
) -> Rc<Vec<TreeNode>> {
    let mut view = KeyView {
        component: component_guid,
        overrides: overrides.unwrap_or_default(),
        derived: derived.unwrap_or_default(),
        size: instance_size.map(ExactSize::from),
        global: cache.global_assignment_epoch,
        epoch: cache.assignment_epoch(component_guid),
    };
    if let Some(children) = cache.get_shared(view) {
        return children;
    }
    let assignments_before = assignment_cache.len();
    let children = Rc::new(apply_instance_overrides_cached(
        symbol_node,
        Some(view.overrides),
        Some(view.derived),
        instance_size,
        assignment_cache,
    ));
    if assignment_cache.len() != assignments_before {
        view.epoch = cache.bump_assignment_epoch(component_guid);
        view.global = cache.global_assignment_epoch;
    }
    cache.insert_shared(view, Rc::clone(&children));
    children
}

// Compatibility helpers are test-only and live beside their private shared implementation.
impl InstanceExpansionCache {
    #[cfg(test)]
    pub(super) fn get(&mut self, key: &InstanceExpansionKey) -> Option<Vec<TreeNode>> {
        self.get_shared(KeyView::from_owned(key))
            .map(|children| children.as_ref().clone())
    }
    #[cfg(test)]
    pub(super) fn insert(&mut self, key: InstanceExpansionKey, children: &[TreeNode]) {
        self.insert_owned(key, Rc::new(children.to_vec()));
    }
}
