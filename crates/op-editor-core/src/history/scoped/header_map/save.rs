//! Reverse a changed-key receipt through borrowed values in canonical key order.
use super::*;
use crate::{canonical_save_receipt::encode, CanonicalHeaderChange, CanonicalHeaderField};
use serde::ser::SerializeMap;
struct Before<'a, V> {
    current: Option<&'a BTreeMap<String, V>>,
    entries: Vec<&'a Entry<V>>,
}
impl<V: Serialize> Serialize for Before<'_, V> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_map(None)?;
        let mut current = self.current.into_iter().flat_map(|m| m.iter()).peekable();
        let mut old = self.entries.iter().copied().peekable();
        while current.peek().is_some() || old.peek().is_some() {
            match (current.peek(), old.peek()) {
                (Some((key, _)), Some(entry)) if *key < &entry.key => {
                    let (key, value) = current.next().unwrap();
                    out.serialize_entry(key, value)?;
                }
                (Some((key, _)), Some(entry)) if *key == &entry.key => {
                    current.next();
                    let entry = old.next().unwrap();
                    if let Some(value) = &entry.old {
                        out.serialize_entry(&entry.key, value)?;
                    }
                }
                (_, Some(_)) => {
                    let entry = old.next().unwrap();
                    if let Some(value) = &entry.old {
                        out.serialize_entry(&entry.key, value)?;
                    }
                }
                (Some(_), None) => {
                    let (key, value) = current.next().unwrap();
                    out.serialize_entry(key, value)?;
                }
                (None, None) => break,
            }
        }
        out.end()
    }
}
impl<V: PartialEq + Serialize> MapPatch<V> {
    pub(in crate::history::scoped) fn canonical_change(
        &self,
        field: CanonicalHeaderField,
        current: &Option<BTreeMap<String, V>>,
        limit: usize,
    ) -> Option<CanonicalHeaderChange> {
        if !self.validate(current)
            || self
                .entries
                .len()
                .checked_mul(std::mem::size_of::<&Entry<V>>())?
                > limit
        {
            return None;
        }
        let before = if self.old_present {
            let mut entries: Vec<_> = self.entries.iter().collect();
            entries.sort_unstable_by(|a, b| a.key.cmp(&b.key));
            if entries.windows(2).any(|pair| pair[0].key == pair[1].key) {
                return None;
            }
            Some(encode(
                &Before {
                    current: current.as_ref(),
                    entries,
                },
                limit,
            )?)
        } else {
            None
        };
        let after = match current {
            Some(value) => Some(encode(value, limit)?),
            None => None,
        };
        Some(CanonicalHeaderChange {
            field,
            before,
            after,
        })
    }
}
