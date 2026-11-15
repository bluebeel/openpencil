//! Borrow the unchanged ledger entries; only the actual token unit is reversed.
use super::*;
use crate::{canonical_save_receipt::encode, CanonicalHeaderChange, CanonicalHeaderField};
use serde::{
    ser::{SerializeSeq, SerializeStruct},
    Serialize,
};
struct Entries<'a> {
    current: &'a [ConversionEntry],
    index: usize,
    old: Option<&'a ConversionEntry>,
}
impl Serialize for Entries<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut out = s.serialize_seq(None)?;
        for (i, value) in self.current.iter().enumerate() {
            if i == self.index {
                if let Some(old) = self.old {
                    out.serialize_element(old)?;
                }
            } else {
                out.serialize_element(value)?;
            }
        }
        out.end()
    }
}
struct Spec<'a> {
    entries: Entries<'a>,
    empty: bool,
}
impl Serialize for Spec<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut out = s.serialize_struct("ConversionSpec", usize::from(!self.empty))?;
        if !self.empty {
            out.serialize_field("entries", &self.entries)?;
        }
        out.end()
    }
}
impl Patch {
    pub(in crate::history::scoped::auxiliary) fn canonical_change(
        &self,
        doc: &PenDocument,
        limit: usize,
    ) -> Option<CanonicalHeaderChange> {
        if self.kind != ConversionKind::Token || !self.validate(doc, false) {
            return None;
        }
        let current = doc.conversion.as_ref()?;
        let ConversionSpec { entries } = current;
        let (index, _) = unit(doc, self.kind, &self.key)??;
        if index != self.index {
            return None;
        }
        let old_len = entries.len().checked_sub(usize::from(self.old.is_none()))?;
        let before = if self.old_present {
            Some(encode(
                &Spec {
                    entries: Entries {
                        current: entries,
                        index,
                        old: self.old.as_ref(),
                    },
                    empty: old_len == 0,
                },
                limit,
            )?)
        } else {
            if old_len != 0 {
                return None;
            }
            None
        };
        Some(CanonicalHeaderChange {
            field: CanonicalHeaderField::Conversion,
            before,
            after: Some(encode(current, limit)?),
        })
    }
}
