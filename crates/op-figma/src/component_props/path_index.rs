//! Indexed legacy path lookup; coordinate coercion matches `same_path`.

use super::{path_value, FigValue};
use std::collections::HashMap;

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct PathKey(Vec<(Option<u64>, Option<u64>)>);

impl PathKey {
    pub(super) fn new<'a>(guids: impl IntoIterator<Item = &'a FigValue>) -> Option<Self> {
        let mut result = Vec::new();
        for guid in guids {
            let coordinate = |name| match guid.get_f64(name) {
                Some(value) if value.is_nan() => None,
                Some(value) => Some(Some(if value == 0.0 {
                    0.0f64.to_bits()
                } else {
                    value.to_bits()
                })),
                None => Some(None),
            };
            result.push((coordinate("sessionID")?, coordinate("localID")?));
        }
        Some(Self(result))
    }
}

#[derive(Default)]
struct Entry {
    first: Option<usize>,
    fields: u8,
}

#[derive(Default)]
pub(super) struct PathIndex(HashMap<PathKey, Entry>);

fn field_bit(field: &str) -> u8 {
    match field {
        "visible" => 1,
        "overriddenSymbolID" => 2,
        "textData" => 4,
        _ => 0,
    }
}

impl PathIndex {
    pub(super) fn authored(overrides: &[FigValue]) -> Self {
        let mut index = Self::default();
        for (position, entry) in overrides.iter().enumerate() {
            let Some(path) = entry
                .get("guidPath")
                .and_then(|path| path.get_array("guids"))
            else {
                continue;
            };
            let Some(key) = PathKey::new(path) else {
                continue;
            };
            let state = index.0.entry(key).or_insert(Entry {
                first: Some(position),
                fields: 0,
            });
            for field in ["visible", "overriddenSymbolID", "textData"] {
                if entry.get(field).is_some() {
                    state.fields |= field_bit(field);
                }
            }
        }
        index
    }

    /// Gate before cloning the target. Any authored duplicate carrying the
    /// field wins; otherwise the first generated assignment wins.
    pub(super) fn accepts(&mut self, key: Option<&PathKey>, field: &str) -> bool {
        let Some(key) = key else {
            return true;
        }; // NaN paths never match.
        let bit = field_bit(field);
        if let Some(state) = self.0.get_mut(key) {
            if state.fields & bit != 0 {
                return false;
            }
            state.fields |= bit;
        } else {
            self.0.insert(
                key.clone(),
                Entry {
                    first: None,
                    fields: bit,
                },
            );
        }
        true
    }

    pub(super) fn apply(
        &mut self,
        overrides: &mut Vec<FigValue>,
        key: Option<PathKey>,
        path: Vec<FigValue>,
        field: &str,
        target: FigValue,
    ) {
        if let Some(state) = key.as_ref().and_then(|key| self.0.get_mut(key)) {
            if let Some(first) = state.first {
                overrides[first].set(field, target);
                return;
            }
            state.first = Some(overrides.len());
        }
        overrides.push(FigValue::Object(vec![
            ("guidPath".into(), path_value(path)),
            (field.into(), target),
        ]));
    }
}
