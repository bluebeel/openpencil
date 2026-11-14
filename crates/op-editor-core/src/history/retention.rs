//! One bounded retained union shared by every actor, with immutable graph reuse.
use super::{History, HISTORY_CAP};
use crate::history_snapshot::CaptureSizeError;
use std::sync::{Arc, Mutex};
#[path = "retention_graph.rs"]
mod graph;
#[path = "retention_slow.rs"]
mod slow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryLimits {
    pub entries: usize,
    pub legacy_entries: usize,
    pub bytes: usize,
}
impl Default for HistoryLimits {
    fn default() -> Self {
        Self {
            entries: 4096,
            legacy_entries: HISTORY_CAP,
            bytes: 2 * 1024 * 1024 * 1024,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryLimitError {
    Entries,
    LegacyEntries,
    Bytes,
}
impl std::fmt::Display for HistoryLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Entries => "history entries must be between 1 and 65536",
            Self::LegacyEntries => {
                "legacy history entries must be between 1 and the shared entry limit"
            }
            Self::Bytes => "history retained bytes must be between 65536 and 4294967296",
        })
    }
}
impl std::error::Error for HistoryLimitError {}
impl HistoryLimits {
    pub fn checked(
        entries: usize,
        legacy_entries: usize,
        bytes: usize,
    ) -> Result<Self, HistoryLimitError> {
        let limits = Self {
            entries,
            legacy_entries,
            bytes,
        };
        limits.validate()?;
        Ok(limits)
    }
    fn validate(self) -> Result<(), HistoryLimitError> {
        if !(1..=65536).contains(&self.entries) {
            return Err(HistoryLimitError::Entries);
        }
        if self.legacy_entries == 0 || self.legacy_entries > self.entries {
            return Err(HistoryLimitError::LegacyEntries);
        }
        if self.bytes < 65536 || self.bytes as u64 > 4 * 1024 * 1024 * 1024u64 {
            return Err(HistoryLimitError::Bytes);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Default)]
pub struct HistoryRetentionStatus {
    pub retained_bytes: usize,
    pub evicted_entries: u64,
    pub sizing_failures: u64,
    pub max_entries: usize,
    pub max_bytes: usize,
}
#[derive(Clone, Default)]
pub(super) struct HistoryRetention {
    cache: Arc<Mutex<graph::HistoryGraphCache>>,
    status: HistoryRetentionStatus,
}
impl std::fmt::Debug for HistoryRetention {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.status.fmt(f)
    }
}
impl HistoryRetention {
    fn measure(&self, history: &History) -> Result<usize, CaptureSizeError> {
        // Sizing computes a conservative allocation count; retention limits
        // apply after the union is known, rather than truncating each member.
        let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        let measured = cache.measure(history, usize::MAX / 2)?;
        match measured {
            Some(bytes) => Ok(bytes.saturating_add(cache.retained_bytes())),
            None => slow::measure(history, usize::MAX / 2),
        }
    }
}
impl History {
    pub fn configure_limits(&mut self, limits: HistoryLimits) -> Result<(), HistoryLimitError> {
        limits.validate()?;
        if self.limits != limits {
            self.limits = limits;
            self.enforce_retention();
        }
        Ok(())
    }
    pub fn limits(&self) -> HistoryLimits {
        self.limits
    }
    pub fn entry_limit(&self) -> usize {
        if self.mixed_actors || self.actor.is_some() {
            self.limits.entries
        } else {
            self.limits.legacy_entries
        }
    }
    pub fn retention_status(&self) -> HistoryRetentionStatus {
        let mut status = self.retention.status;
        status.max_entries = self.entry_limit();
        status.max_bytes = self.limits.bytes;
        status
    }
    /// Owned runtime metadata copied by a transaction's History::clone.
    pub fn runtime_clone_bytes(&self) -> usize {
        std::mem::size_of::<Self>().saturating_add(self.conflicts.len().saturating_mul(64))
    }
    /// The immutable graph index is shared by cloned History values; count its
    /// actual retained allocation once in the authority's resident ledger.
    pub fn runtime_storage_bytes(&self) -> usize {
        self.runtime_clone_bytes().saturating_add(
            self.retention
                .cache
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .retained_bytes(),
        )
    }
    fn evict_oldest(&mut self) -> bool {
        let past = self.past.front().map(|entry| entry.history_tag().operation);
        let future = self
            .future
            .front()
            .map(|entry| entry.history_tag().operation);
        let removed = match (past, future) {
            (Some(a), Some(b)) if a <= b => self.past.pop_front(),
            (Some(_), Some(_)) => self.future.pop_front(),
            (Some(_), None) => self.past.pop_front(),
            (None, Some(_)) => self.future.pop_front(),
            _ => None,
        };
        if let Some(entry) = removed {
            self.conflicts.remove(&entry.history_tag().operation);
            self.retention.status.evicted_entries =
                self.retention.status.evicted_entries.saturating_add(1);
            true
        } else {
            false
        }
    }
    pub(crate) fn enforce_retention(&mut self) {
        let count = self.entry_limit();
        while self.past.len() + self.future.len() > count {
            self.evict_oldest();
        }
        loop {
            match self.retention.measure(self) {
                Ok(bytes) if bytes <= self.limits.bytes => {
                    self.retention.status.retained_bytes = bytes;
                    break;
                }
                Ok(_) => {
                    if !self.evict_oldest() {
                        self.retention.status.retained_bytes = 0;
                        break;
                    }
                }
                Err(_) => {
                    // Never retain unaccounted graphs. A sizing failure trims
                    // this shared ledger, not canonical edits or other files.
                    self.retention.status.sizing_failures =
                        self.retention.status.sizing_failures.saturating_add(1);
                    if !self.evict_oldest() {
                        self.retention.status.retained_bytes = 0;
                        break;
                    }
                }
            }
        }
        self.retention.status.max_entries = count;
        self.retention.status.max_bytes = self.limits.bytes;
    }
}
