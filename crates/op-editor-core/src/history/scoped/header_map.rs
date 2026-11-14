//! Exact one-sided deltas for optional persisted header maps.
use serde::Serialize;
use std::{collections::BTreeMap, mem};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct MapPatch<V> {
    old_present: bool,
    expected_present: bool,
    expected_len: usize,
    entries: Vec<Entry<V>>,
}
#[derive(Debug, Clone, PartialEq)]
struct Entry<V> {
    key: String,
    old: Option<V>,
    expected_present: bool,
}
pub(super) fn exact<T: PartialEq + Serialize + ?Sized>(a: &T, b: &T) -> bool {
    a == b
        && crate::history_snapshot::metadata_sensitivity(a)
            == crate::history_snapshot::metadata_sensitivity(b)
}
impl<V: PartialEq + Serialize> MapPatch<V> {
    pub(super) fn compact(
        old: Option<BTreeMap<String, V>>,
        current: &Option<BTreeMap<String, V>>,
    ) -> Option<Self> {
        let old_present = old.is_some();
        let mut old = old.unwrap_or_default();
        let mut entries = Vec::new();
        if let Some(current) = current {
            for (key, value) in current {
                match old.remove(key) {
                    Some(previous) if exact(&previous, value) => {}
                    previous => entries.push(Entry {
                        key: key.clone(),
                        old: previous,
                        expected_present: true,
                    }),
                }
            }
        }
        entries.extend(old.into_iter().map(|(key, old)| Entry {
            key,
            old: Some(old),
            expected_present: false,
        }));
        if entries.is_empty() && old_present == current.is_some() {
            return None;
        }
        entries.shrink_to_fit();
        Some(Self {
            old_present,
            expected_present: current.is_some(),
            expected_len: current.as_ref().map_or(0, BTreeMap::len),
            entries,
        })
    }
    pub(super) fn validate(&self, current: &Option<BTreeMap<String, V>>) -> bool {
        self.expected_present == current.is_some()
            && self.expected_len == current.as_ref().map_or(0, BTreeMap::len)
            && self.entries.iter().all(|entry| {
                current
                    .as_ref()
                    .is_some_and(|map| map.contains_key(&entry.key))
                    == entry.expected_present
            })
    }
    /// Validation is performed for every header field before any mutation.
    pub(super) fn exchange(&mut self, current: &mut Option<BTreeMap<String, V>>) {
        let present = current.is_some();
        let target_present = self.old_present;
        let map = current.get_or_insert_with(BTreeMap::new);
        for entry in &mut self.entries {
            entry.expected_present = entry.old.is_some();
            entry.old = match entry.old.take() {
                Some(old) => map.insert(entry.key.clone(), old),
                None => map.remove(&entry.key),
            };
        }
        let target_len = map.len();
        if !mem::replace(&mut self.old_present, present) {
            debug_assert!(map.is_empty());
            *current = None;
        }
        self.expected_present = target_present;
        self.expected_len = target_len;
    }
    #[cfg(test)]
    pub(super) fn retained_entries(&self) -> usize {
        self.entries.len()
    }
    pub(super) fn storage_bytes(&self) -> usize {
        let nested = self
            .entries
            .iter()
            .map(|entry| {
                entry
                    .key
                    .capacity()
                    .saturating_add(super::header::retained_bytes(&entry.old))
            })
            .fold(0usize, usize::saturating_add);
        self.entries
            .capacity()
            .saturating_mul(mem::size_of::<Entry<V>>())
            .saturating_add(nested)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changed_keys_only_preserve_optional_maps_and_reverse_values() {
        let old = Some(BTreeMap::from([
            ("same".into(), 9.0f64),
            ("zero".into(), -0.0),
        ]));
        let mut current = Some(BTreeMap::from([
            ("same".into(), 9.0),
            ("zero".into(), 0.0),
            ("new".into(), 2.0),
        ]));
        let mut patch = MapPatch::compact(old.clone(), &current).unwrap();
        assert_eq!(patch.retained_entries(), 2);
        let after = current.clone();
        assert!(patch.validate(&current));
        patch.exchange(&mut current);
        assert!(exact(&current, &old));
        assert!(patch.validate(&current));
        patch.exchange(&mut current);
        assert!(exact(&current, &after));
        for (old, mut current) in [
            (None, Some(BTreeMap::<String, f64>::new())),
            (Some(BTreeMap::new()), None),
        ] {
            let after = current.clone();
            let mut patch = MapPatch::compact(old.clone(), &current).unwrap();
            patch.exchange(&mut current);
            assert!(exact(&current, &old));
            assert!(patch.validate(&current));
            patch.exchange(&mut current);
            assert!(exact(&current, &after));
        }
    }
}
