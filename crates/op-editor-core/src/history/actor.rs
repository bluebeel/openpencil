//! Runtime-only ownership and selective history. Never serialized into a document.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HistoryActor(u64);
impl HistoryActor {
    /// Opaque process-local identity; callers retain one per admitted actor.
    pub fn allocate() -> Option<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
            next.checked_add(1)
        })
        .ok()
        .map(Self)
    }
}
#[derive(Debug, Clone, Copy, Default)]
pub struct HistoryTag {
    pub(crate) actor: Option<HistoryActor>,
    pub(crate) operation: u64,
}
// Value equality of snapshots remains content/owner based. The opaque token
// is used only for branch bookkeeping, not canonical snapshot comparisons.
impl PartialEq for HistoryTag {
    fn eq(&self, other: &Self) -> bool {
        self.actor == other.actor
    }
}
impl Eq for HistoryTag {}
impl HistoryTag {
    pub(crate) fn capture(actor: Option<HistoryActor>) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let operation = NEXT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                next.checked_add(1)
            })
            .unwrap_or(0);
        Self { actor, operation }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActorHistoryError {
    Empty {
        redo: bool,
    },
    Conflict {
        operation: u64,
        blocker: Option<u64>,
    },
    ScopeChanged {
        operation: u64,
    },
}
impl std::fmt::Display for ActorHistoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty { redo } => write!(f, "actor-history-empty: no retained {} operation belongs to this agent", if *redo {"redo"} else {"undo"}),
            Self::Conflict { operation, blocker: Some(blocker) } => write!(f, "actor-history-conflict: captured scope for operation {operation} overlaps later operation {blocker}; neither was changed"),
            Self::Conflict { operation, blocker: None } => write!(f, "actor-history-conflict: operation {operation} redo overlaps a subsequent edit; no operation was changed"),
            Self::ScopeChanged { operation } => write!(f, "actor-history-scope-changed: operation {operation} no longer matches its captured destinations; no operation was changed"),
        }
    }
}
impl std::error::Error for ActorHistoryError {}
impl HistoryEntry {
    pub fn history_tag(&self) -> HistoryTag {
        match self {
            Self::Full(snapshot) => snapshot.history_tag,
            Self::Scoped(snapshot) => snapshot.history_tag,
        }
    }
    pub(crate) fn conflicts_with(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Scoped(a), Self::Scoped(b)) => a.actor_conflicts(b),
            _ => true,
        }
    }
    pub(crate) fn selective(&self) -> bool {
        self.scoped_snapshot()
            .is_some_and(|snapshot| snapshot.actor_safe)
    }
}
impl History {
    pub fn actor(&self) -> Option<HistoryActor> {
        self.actor
    }
    pub fn actor_depths(&self) -> (usize, usize) {
        let count = |entries: &VecDeque<HistoryEntry>| match self.actor {
            Some(actor) => entries
                .iter()
                .filter(|entry| entry.history_tag().actor == Some(actor))
                .count(),
            None => entries.len(),
        };
        (count(&self.past), count(&self.future))
    }
    pub(crate) fn next_index(&self, redo: bool) -> Option<usize> {
        let entries = if redo { &self.future } else { &self.past };
        match self.actor {
            Some(actor) => entries
                .iter()
                .rposition(|entry| entry.history_tag().actor == Some(actor)),
            None => entries.len().checked_sub(1),
        }
    }
    pub(crate) fn next_entry(&self, redo: bool) -> Option<&HistoryEntry> {
        let index = self.next_index(redo)?;
        (if redo { &self.future } else { &self.past }).get(index)
    }
    pub(crate) fn validate_actor(&self, redo: bool) -> Result<(), ActorHistoryError> {
        let index = self
            .next_index(redo)
            .ok_or(ActorHistoryError::Empty { redo })?;
        let entry = (if redo { &self.future } else { &self.past })
            .get(index)
            .unwrap();
        let operation = entry.history_tag().operation;
        if redo {
            if self.conflicts.contains(&operation) {
                return Err(ActorHistoryError::Conflict {
                    operation,
                    blocker: None,
                });
            }
        } else if self.actor.is_some() {
            if let Some(blocker) = self
                .past
                .iter()
                .skip(index + 1)
                .find(|later| entry.conflicts_with(later))
            {
                return Err(ActorHistoryError::Conflict {
                    operation,
                    blocker: Some(blocker.history_tag().operation),
                });
            }
        }
        Ok(())
    }
    /// New writes discard only their actor's redo branch. Other actor branches
    /// remain available, with overlapping changes explicitly marked conflicted.
    pub(crate) fn invalidate_redo(&mut self, changed: &HistoryEntry, discard_own: bool) {
        let owner = changed.history_tag().actor;
        if discard_own {
            self.future
                .retain(|entry| entry.history_tag().actor != owner);
        }
        for entry in &self.future {
            if entry.history_tag().actor != owner && changed.conflicts_with(entry) {
                self.conflicts.insert(entry.history_tag().operation);
            }
        }
        self.prune_conflicts();
    }
    pub(crate) fn prune_conflicts(&mut self) {
        let live: std::collections::HashSet<_> = self
            .future
            .iter()
            .map(|entry| entry.history_tag().operation)
            .collect();
        self.conflicts.retain(|operation| live.contains(operation));
    }
}
impl EditorState {
    /// Host scopes install an opaque actor and restore the prior context on exit.
    pub fn set_history_actor(&mut self, actor: Option<HistoryActor>) -> Option<HistoryActor> {
        std::mem::replace(&mut self.history.actor, actor)
    }
    pub fn validate_actor_history(&self, redo: bool) -> Result<(), ActorHistoryError> {
        self.history.validate_actor(redo)
    }
    pub fn take_actor_history_error(&mut self) -> Option<ActorHistoryError> {
        self.history.actor_error.take()
    }
    pub(crate) fn exchange_history(&mut self, redo: bool) -> bool {
        self.exchange_history_retained(redo).0
    }
    pub(crate) fn exchange_history_retained(
        &mut self,
        redo: bool,
    ) -> (bool, Option<Arc<ScopedEditorSnapshot>>) {
        if let Err(error) = self.history.validate_actor(redo) {
            if self.history.actor.is_some()
                || redo && !matches!(error, ActorHistoryError::Empty { .. })
            {
                self.history.actor_error = Some(error);
            }
            return (false, None);
        }
        self.history.actor_error = None;
        let index = self.history.next_index(redo).unwrap();
        let mut entry = (if redo {
            &mut self.history.future
        } else {
            &mut self.history.past
        })
        .remove(index)
        .unwrap();
        let operation = entry.history_tag().operation;
        let selective = entry.selective()
            && (self.history.actor.is_some() || entry.history_tag().actor.is_some());
        let succeeded = match entry {
            HistoryEntry::Full(previous) => {
                let mut current = self.snapshot_for_history_with_anchor(Some(&previous));
                current.history_tag = previous.history_tag;
                self.restore(previous);
                entry = HistoryEntry::Full(current);
                true
            }
            HistoryEntry::Scoped(previous) => {
                let mut snapshot = Arc::unwrap_or_clone(previous);
                let exchanged = if selective {
                    snapshot.exchange_actor(self)
                } else {
                    snapshot.exchange(self)
                };
                entry = HistoryEntry::Scoped(Arc::new(snapshot));
                exchanged.is_ok()
            }
        };
        if !succeeded {
            (if redo {
                &mut self.history.future
            } else {
                &mut self.history.past
            })
            .insert(index, entry);
            self.history.actor_error = Some(ActorHistoryError::ScopeChanged { operation });
            return (false, None);
        }
        let reverse = entry.scoped_snapshot().cloned();
        self.history.invalidate_redo(&entry, false);
        self.history.conflicts.remove(&operation);
        (if redo {
            &mut self.history.past
        } else {
            &mut self.history.future
        })
        .push_back(entry);
        if !selective {
            let generation = self.document_generation;
            rebase_scoped_prefix(
                if redo {
                    &mut self.history.future
                } else {
                    &mut self.history.past
                },
                generation,
            );
        }
        self.history.enforce_retention();
        (true, reverse)
    }
}
