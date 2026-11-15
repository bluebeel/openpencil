//! Validate candidate keys against borrowed authority inputs outside the cache
//! lock. Scene/index construction and large key destruction stay outside it too.
use super::*;
fn input(key: &Key) -> Option<&SceneInputs> {
    match key {
        Key::Scene(input) | Key::Export { input, .. } | Key::Geometry { input, .. } => Some(input),
        _ => None,
    }
}
pub(super) fn hit(state: &EditorState, kind: impl Fn(&Key) -> bool) -> Option<Value> {
    let mut dead = Vec::new();
    let mut slots = Vec::new();
    let candidates = {
        let mut cache = store().lock().unwrap_or_else(|e| e.into_inner());
        cache.purge(&mut dead, &mut slots);
        cache
            .entries
            .iter()
            .rev()
            .filter(|entry| {
                kind(&entry.key) && input(&entry.key).is_some_and(|i| i.same_authority(state))
            })
            .map(|entry| (Arc::clone(&entry.key), entry.value.clone()))
            .collect::<Vec<_>>()
    };
    drop(dead);
    drop(slots);
    for (key, value) in candidates {
        if !input(&key).is_some_and(|input| input.matches_state(state)) {
            continue;
        }
        // A concurrent eviction cannot invalidate an immutable same-input
        // result. Its Arc is still in the existing retired-pin byte ledger.
        let mut cache = store().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(at) = cache
            .entries
            .iter()
            .position(|entry| Arc::ptr_eq(&entry.key, &key))
        {
            let entry = cache.entries.remove(at);
            cache.entries.push(entry);
        }
        cache.hits = cache.hits.saturating_add(1);
        drop(cache);
        return Some(value);
    }
    None
}
