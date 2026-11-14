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
    pub(super) fn single_key(
        old_present: bool,
        old_len: usize,
        key: String,
        old: Option<V>,
        current: &Option<BTreeMap<String, V>>,
    ) -> Option<Self> {
        let value = current.as_ref().and_then(|map| map.get(&key));
        if old_present == current.is_some() && exact(&old.as_ref(), &value) {
            return None;
        }
        let old_key_present = old.is_some();
        let expected_present = value.is_some();
        let expected_len = old_len
            .saturating_add(usize::from(expected_present && !old_key_present))
            .saturating_sub(usize::from(old_key_present && !expected_present));
        Some(Self {
            old_present,
            expected_present: current.is_some(),
            expected_len,
            entries: vec![Entry {
                key,
                old,
                expected_present,
            }],
        })
    }
    pub(super) fn actor_safe(&self) -> bool {
        self.old_present && self.expected_present
    }
    pub(super) fn actor_conflicts(&self, other: &Self) -> bool {
        !self.actor_safe()
            || !other.actor_safe()
            || self
                .entries
                .iter()
                .any(|a| other.entries.iter().any(|b| a.key == b.key))
    }
    pub(super) fn validate_actor(&self, current: &Option<BTreeMap<String, V>>) -> bool {
        self.actor_safe()
            && current.is_some()
            && self.entries.iter().all(|entry| {
                current
                    .as_ref()
                    .is_some_and(|map| map.contains_key(&entry.key))
                    == entry.expected_present
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
    /// Values moved from the live map become the reverse receipt. Inspect
    /// only changed keys, retaining all typed fields and exact optional shape.
    pub(super) fn cohort_live_bound(
        &self,
        current: &Option<BTreeMap<String, V>>,
        limit: usize,
    ) -> Option<usize> {
        let mut bytes = self
            .entries
            .len()
            .checked_mul(mem::size_of::<Entry<V>>() + 256)?;
        for entry in &self.entries {
            let value = current.as_ref().and_then(|map| map.get(&entry.key));
            bytes = bytes.checked_add(
                crate::history_snapshot::command_capture_value_bytes(
                    &value,
                    limit.checked_sub(bytes)?,
                )
                .ok()?,
            )?;
            bytes = bytes.checked_add(entry.key.capacity().checked_mul(4)?)?;
        }
        (bytes <= limit).then_some(bytes)
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
