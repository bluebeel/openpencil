//! Bounded measurements of immutable shared undo roots.
//!
//! Weak ownership cannot retain a removed subtree's strings/children. It also
//! prevents address reuse while an entry exists, and Arc::make_mut detaches a
//! weak-only allocation before changing it, so cached sizes never describe a
//! different or subsequently mutated root at the same address.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Weak},
};

const MAX_ENTRIES: usize = 16_384;

#[derive(Clone, Copy, Default)]
pub(super) struct RootCharge {
    pub serialized: usize,
    pub nodes: usize,
    pub shared_bytes: usize,
}

struct Entry<T> {
    root: Weak<T>,
    charge: RootCharge,
}

pub(super) struct RootCache<T> {
    entries: HashMap<usize, Entry<T>>,
}

impl<T> Default for RootCache<T> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}

impl<T> RootCache<T> {
    pub fn prune(&mut self) {
        self.entries
            .retain(|_, entry| entry.root.strong_count() != 0);
        if self.entries.is_empty() {
            self.entries.shrink_to_fit();
        }
    }

    pub fn get(&self, root: &Arc<T>) -> Option<RootCharge> {
        let key = Arc::as_ptr(root) as usize;
        self.entries.get(&key).map(|entry| entry.charge)
    }

    pub fn insert(&mut self, root: &Arc<T>, charge: RootCharge) {
        if self.entries.len() < MAX_ENTRIES {
            self.entries.insert(
                Arc::as_ptr(root) as usize,
                Entry {
                    root: Arc::downgrade(root),
                    charge,
                },
            );
        }
    }

    pub fn retain_visited(&mut self, visited: &HashSet<usize>) {
        self.entries.retain(|key, _| visited.contains(key));
        if self.entries.is_empty() {
            self.entries.shrink_to_fit();
        }
    }

    /// Live T slots are charged by the owning history root. After retaining
    /// only this ledger's visited roots, its borrowed state keeps those roots
    /// alive throughout measurement. Include a dead weak slot for other uses.
    pub fn retained_bytes(&self) -> usize {
        self.entries
            .capacity()
            .saturating_mul(std::mem::size_of::<(usize, Entry<T>)>() + 16)
            .saturating_add(self.entries.values().fold(0usize, |total, entry| {
                total
                    .saturating_add(2 * std::mem::size_of::<usize>())
                    .saturating_add(if entry.root.strong_count() == 0 {
                        std::mem::size_of::<T>()
                    } else {
                        0
                    })
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weak_measurements_detach_on_mutation_and_release_removed_payload() {
        let mut cache = RootCache::default();
        let mut root = Arc::new(vec![0_u8; 4096]);
        cache.insert(
            &root,
            RootCharge {
                serialized: 5000,
                nodes: 1,
                shared_bytes: 0,
            },
        );
        assert_eq!(cache.get(&root).unwrap().serialized, 5000);
        Arc::make_mut(&mut root).resize(8192, 1);
        assert!(cache.get(&root).is_none());
        cache.prune();
        assert_eq!(cache.retained_bytes(), 0);
        assert_eq!(Arc::weak_count(&root), 0);
    }

    #[test]
    fn entries_stay_bounded_without_retaining_strong_roots() {
        let roots: Vec<_> = (0..MAX_ENTRIES + 100).map(Arc::new).collect();
        let mut cache = RootCache::default();
        for root in &roots {
            cache.insert(
                root,
                RootCharge {
                    serialized: 1,
                    nodes: 1,
                    shared_bytes: 0,
                },
            );
        }
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
        assert!(roots.iter().all(|root| Arc::strong_count(root) == 1));
        drop(roots);
        cache.prune();
        assert_eq!(cache.retained_bytes(), 0);
    }
}
