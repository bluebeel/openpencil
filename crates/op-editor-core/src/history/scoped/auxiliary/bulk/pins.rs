//! Restore only the pruning this command performed, not a later actor's view.
use std::collections::BTreeMap;
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Pin {
    key: String,
    old: Option<String>,
    expected: Option<String>,
    active: bool,
}
pub(super) fn compact(
    old: BTreeMap<String, String>,
    current: &BTreeMap<String, String>,
) -> Vec<Pin> {
    let mut patches = Vec::new();
    for (key, value) in old {
        if current.get(&key) != Some(&value) {
            patches.push(Pin {
                expected: current.get(&key).cloned(),
                key,
                old: Some(value),
                active: true,
            });
        }
    }
    patches
}
impl Pin {
    pub(super) fn bytes(&self) -> usize {
        self.key.capacity()
            + self.old.as_ref().map_or(0, String::capacity)
            + self.expected.as_ref().map_or(0, String::capacity)
    }
}
pub(super) fn exchange(
    patches: &mut [Pin],
    current: &mut BTreeMap<String, String>,
    themes: Option<&BTreeMap<String, Vec<String>>>,
) {
    for patch in patches.iter_mut() {
        if !patch.active {
            continue;
        }
        if current.get(&patch.key) != patch.expected.as_ref() {
            patch.active = false;
            patch.old = None;
            patch.expected = None;
            continue;
        }
        let previous = match patch.old.take() {
            Some(value) => current.insert(patch.key.clone(), value),
            None => current.remove(&patch.key),
        };
        patch.old = previous;
        patch.expected = current.get(&patch.key).cloned();
    }
    current.retain(|axis, value| {
        themes
            .and_then(|t| t.get(axis))
            .is_some_and(|values| values.contains(value))
    });
    // If validity removed a restored pin, retain the actual opposite witness.
    for patch in patches.iter_mut() {
        if patch.active {
            patch.expected = current.get(&patch.key).cloned();
        }
    }
}
