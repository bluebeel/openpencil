//! One exact conversion ledger unit, rebased by semantic identity during actor undo.
use super::{exact, ScopedHistoryError};
use jian_ops_schema::{
    conversion::{ConversionEntry, ConversionKind, ConversionSpec},
    PenDocument,
};
use std::mem;
mod save;
fn unit<'a>(
    doc: &'a PenDocument,
    kind: ConversionKind,
    key: &str,
) -> Option<Option<(usize, &'a ConversionEntry)>> {
    let Some(spec) = &doc.conversion else {
        return Some(None);
    };
    let mut entries = spec
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.kind == kind && e.key == key);
    let first = entries.next();
    if entries.next().is_some() {
        None
    } else {
        Some(first)
    }
}
pub(in crate::history::scoped::auxiliary) fn forecast(
    doc: &PenDocument,
    key: &str,
    bytes: &mut usize,
    limit: usize,
) -> Option<()> {
    forecast_kind(doc, ConversionKind::Token, key, bytes, limit)
}
pub(in crate::history::scoped::auxiliary) fn forecast_kind(
    doc: &PenDocument,
    kind: ConversionKind,
    key: &str,
    bytes: &mut usize,
    limit: usize,
) -> Option<()> {
    // A token append/redo may relocate the Vec backing allocation. Charge
    // slots as well as the changed entry, without cloning unrelated entries.
    *bytes = bytes.checked_add(
        doc.conversion
            .as_ref()
            .map_or(0, |c| c.entries.capacity())
            .checked_mul(std::mem::size_of::<ConversionEntry>() + 64)?,
    )?;
    let value = unit(doc, kind, key)?.map(|(_, value)| value);
    *bytes = bytes.checked_add(
        crate::history_snapshot::command_capture_value_bytes(&value, limit.checked_sub(*bytes)?)
            .ok()?,
    )?;
    *bytes = bytes.checked_add(key.len().checked_mul(8)?)?;
    (*bytes <= limit).then_some(())
}
#[derive(Debug)]
pub(in crate::history::scoped::auxiliary) struct Pending {
    kind: ConversionKind,
    key: String,
    old: Option<ConversionEntry>,
    index: usize,
    present: bool,
    total: usize,
}
#[derive(Debug, Clone, PartialEq)]
pub(in crate::history::scoped::auxiliary) struct Patch {
    kind: ConversionKind,
    key: String,
    old: Option<ConversionEntry>,
    expected: Option<ConversionEntry>,
    index: usize,
    old_present: bool,
    expected_present: bool,
    expected_total: usize,
}
impl Pending {
    pub(in crate::history::scoped::auxiliary) fn capture(
        doc: &PenDocument,
        key: &str,
    ) -> Option<Self> {
        Self::capture_kind(doc, ConversionKind::Token, key)
    }
    pub(in crate::history::scoped::auxiliary) fn capture_kind(
        doc: &PenDocument,
        kind: ConversionKind,
        key: &str,
    ) -> Option<Self> {
        let entry = unit(doc, kind, key)?;
        let total = doc.conversion.as_ref().map_or(0, |c| c.entries.len());
        Some(Self {
            kind,
            key: key.into(),
            old: entry.map(|(_, value)| value.clone()),
            index: entry.map_or(total, |(index, _)| index),
            present: doc.conversion.is_some(),
            total,
        })
    }
    pub(in crate::history::scoped::auxiliary) fn matches(&self, doc: &PenDocument) -> bool {
        self.present == doc.conversion.is_some()
            && self.total == doc.conversion.as_ref().map_or(0, |c| c.entries.len())
            && unit(doc, self.kind, &self.key)
                .is_some_and(|entry| exact(&self.old.as_ref(), &entry.map(|(_, value)| value)))
    }
    pub(in crate::history::scoped::auxiliary) fn compact(
        self,
        doc: &PenDocument,
    ) -> Result<Option<Patch>, ScopedHistoryError> {
        let entry =
            unit(doc, self.kind, &self.key).ok_or(ScopedHistoryError::DocumentHeaderChanged)?;
        let expected = entry.map(|(_, value)| value.clone());
        if self.present == doc.conversion.is_some() && exact(&self.old, &expected) {
            return Ok(None);
        }
        let total = self.total + usize::from(self.old.is_none() && expected.is_some());
        if doc.conversion.as_ref().map_or(0, |c| c.entries.len()) != total {
            return Err(ScopedHistoryError::DocumentHeaderChanged);
        }
        Ok(Some(Patch {
            kind: self.kind,
            key: self.key,
            old: self.old,
            expected,
            index: self.index,
            old_present: self.present,
            expected_present: doc.conversion.is_some(),
            expected_total: total,
        }))
    }
}
impl Patch {
    pub(in crate::history::scoped::auxiliary) fn actor_safe(&self) -> bool {
        self.old_present && self.expected_present
    }
    pub(in crate::history::scoped::auxiliary) fn conflicts(&self, other: &Self) -> bool {
        !self.actor_safe()
            || !other.actor_safe()
            || (self.kind == other.kind && self.key == other.key)
    }
    pub(in crate::history::scoped::auxiliary) fn validate(
        &self,
        doc: &PenDocument,
        actor: bool,
    ) -> bool {
        let shape = if actor {
            self.actor_safe() && doc.conversion.is_some()
        } else {
            self.expected_present == doc.conversion.is_some()
                && self.expected_total == doc.conversion.as_ref().map_or(0, |c| c.entries.len())
        };
        shape
            && unit(doc, self.kind, &self.key)
                .is_some_and(|entry| exact(&self.expected.as_ref(), &entry.map(|(_, value)| value)))
    }
    pub(in crate::history::scoped::auxiliary) fn exchange(&mut self, doc: &mut PenDocument) {
        let previous_present = doc.conversion.is_some();
        let target_present = self.old_present;
        let spec = doc.conversion.get_or_insert_with(ConversionSpec::default);
        let at = spec
            .entries
            .iter()
            .position(|e| e.kind == self.kind && e.key == self.key);
        let replacement = self.old.take();
        self.old = match (at, replacement) {
            (Some(at), Some(value)) => {
                self.index = at;
                Some(mem::replace(&mut spec.entries[at], value))
            }
            (Some(at), None) => {
                self.index = at;
                Some(spec.entries.remove(at))
            }
            (None, Some(value)) => {
                let at = self.index.min(spec.entries.len());
                spec.entries.insert(at, value);
                self.index = at;
                None
            }
            (None, None) => None,
        };
        self.expected = spec
            .entries
            .iter()
            .find(|e| e.kind == self.kind && e.key == self.key)
            .cloned();
        self.expected_total = spec.entries.len();
        self.old_present = previous_present;
        self.expected_present = target_present;
        if !target_present {
            debug_assert!(spec.entries.is_empty());
            doc.conversion = None;
        }
    }
    pub(in crate::history::scoped::auxiliary) fn bytes(&self) -> usize {
        mem::size_of::<Self>()
            + self.key.capacity()
            + super::super::super::header::retained_bytes(&self.old)
            + super::super::super::header::retained_bytes(&self.expected)
    }
    pub(in crate::history::scoped::auxiliary) fn live_bound(
        &self,
        doc: &PenDocument,
        limit: usize,
    ) -> Option<usize> {
        let slots = doc
            .conversion
            .as_ref()
            .map_or(0, |c| c.entries.capacity())
            .checked_mul(std::mem::size_of::<ConversionEntry>() + 64)?;
        let value = unit(doc, self.kind, &self.key)?.map(|(_, value)| value);
        slots.checked_add(
            crate::history_snapshot::command_capture_value_bytes(&value, limit.checked_sub(slots)?)
                .ok()?,
        )
    }
}
