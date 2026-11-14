//! Authority-local numeric seeding. Nothing is cached on a public document
//! outside a synchronous authority callback. The owner must hold the same exclusive writer
//! authority and route every canonical mutation through observed typed commands.
use crate::{EditorCommand as C, EditorState, IdAllocError};
use std::{
    mem,
    panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
    sync::{Mutex, OnceLock},
};

#[path = "command_id_lookup.rs"]
mod lookup;
pub(crate) use lookup::Order as LookupOrder;
type Seed = Result<u64, IdAllocError>;

/// One receipt owned by a locked cohort, never by an agent, revision or file
/// cache. Keep it only while the same exclusive authority is held, and route
/// all mutations through its callbacks. Dropping/releasing that authority ends
/// the receipt. Arbitrary mutations between callbacks must invalidate it too;
/// normal public editing is unchanged.
#[derive(Debug, Default)]
pub struct CommandIdSequence {
    owner: Option<(usize, u64)>,
    next: Option<Seed>,
    lookup: Option<lookup::Index>,
}

impl CommandIdSequence {
    pub fn invalidate(&mut self) {
        self.next = None;
        self.lookup = None;
    }

    /// The same provenance contract as a trusted command capture: the caller
    /// owns the complete editor/actor authority throughout this synchronous
    /// callback. Registry reads and actor swaps may borrow it; document writes
    /// must use typed apply/capture entrypoints, or explicitly invalidate first.
    /// Its address/generation tag is only an authority sanity check; it is
    /// never dereferenced and cannot authorize mutation outside this scope.
    pub fn with_authority<R>(
        &mut self,
        state: &mut EditorState,
        operation: impl FnOnce(&mut EditorState) -> R,
    ) -> R {
        // Nested authority callbacks do not inherit another owner's proof.
        if state.command_id_seed.entered {
            self.invalidate();
            state.invalidate_command_id_sequence();
            return operation(state);
        }
        let owner = (
            state as *const EditorState as usize,
            state.document_generation(),
        );
        if self.owner != Some(owner) {
            self.owner = Some(owner);
            self.invalidate();
        }
        let prior = mem::replace(
            &mut state.command_id_seed,
            SeedOverride::new(owner.1, self.next, self.lookup.take()),
        );
        let result = catch_unwind(AssertUnwindSafe(|| operation(state)));
        let mut observed = mem::replace(&mut state.command_id_seed, prior);
        self.next = None;
        self.lookup = None;
        if result.is_ok() && observed.valid && state.document_generation() == owner.1 {
            self.lookup = observed.take_lookup(state);
            if let Some(seed) = observed.next.get().copied() {
                let runtime = state.runtime_next_id_seed();
                // A public pre-scope write may have introduced a higher live
                // numeric id without advancing runtime ownership. Deletion may
                // remove it, so retain that seed only once runtime covers it.
                let covered = match (seed, runtime) {
                    (_, Err(_)) => true,
                    (Ok(seed), Ok(runtime)) => runtime >= seed,
                    (Err(_), Ok(_)) => false,
                };
                if !observed.removes || covered {
                    self.next = Some(match (seed, runtime) {
                        (Ok(seed), Ok(runtime)) => Ok(seed.max(runtime)),
                        _ => Err(IdAllocError::CounterExhausted),
                    });
                }
            }
        }
        match result {
            Ok(value) => value,
            Err(panic) => resume_unwind(panic),
        }
    }
}

/// Installed only during one synchronous authority callback. Cloning an editor
/// deliberately drops this ephemeral proof, even while the original is active.
#[derive(Debug, Default)]
pub(crate) struct SeedOverride {
    entered: bool,
    generation: u64,
    valid: bool,
    removes: bool,
    next: OnceLock<Seed>,
    lookup: Mutex<lookup::Cache>,
}
impl Clone for SeedOverride {
    fn clone(&self) -> Self {
        Self::default()
    }
}
impl SeedOverride {
    fn new(generation: u64, next: Option<Seed>, index: Option<lookup::Index>) -> Self {
        let slot = OnceLock::new();
        if let Some(next) = next {
            let _ = slot.set(next);
        }
        Self {
            entered: true,
            generation,
            valid: true,
            removes: false,
            next: slot,
            lookup: Mutex::new(lookup::Cache::new(index)),
        }
    }
    pub(crate) fn get(&self, state: &EditorState) -> Option<Seed> {
        if !self.entered || !self.valid || self.generation != state.document_generation() {
            return None;
        }
        let seed = *self
            .next
            .get_or_init(|| crate::id_allocator::next_sequential_counter(&state.doc));
        Some(seed.and_then(|next| {
            state
                .runtime_next_id_seed()
                .map(|runtime| next.max(runtime))
        }))
    }
    fn observe(&mut self, command: &C) {
        if !self.entered || !self.valid {
            return;
        }
        match command {
            C::Batch { commands } => {
                // Core rejects nested Batch before execution. Do not recurse
                // through a rejected, arbitrarily deep caller-owned value.
                if commands
                    .iter()
                    .any(|command| matches!(command, C::Batch { .. }))
                {
                    self.valid = false;
                } else {
                    for command in commands {
                        self.observe(command);
                    }
                }
            }
            C::PatchNodeData { .. }
                if crate::command_node_attrs::bulk::prepare(command).is_some() =>
            {
                ()
            }
            // These commands mint/remap all outputs through the runtime
            // observer, or publish the accepted authored numeric maximum.
            C::InsertNode { .. }
            | C::CopyNode { .. }
            | C::ReplaceNode { .. }
            | C::ReplaceSubtree { .. }
            | C::BatchInsert { .. }
            | C::InsertSubtree { .. }
            | C::InsertAuthoredSubtree { .. }
            | C::InsertAuthoredSubtreePreservingRoots { .. }
            | C::DuplicateSelected { .. }
            | C::PasteClipboard { .. }
            | C::GroupSelected
            | C::AddPage { .. }
            | C::DuplicatePage { .. }
            | C::InstantiateComponent { .. }
            | C::InstantiateKitComponent { .. }
            | C::ImportSvg { .. } => {
                self.removes |= matches!(
                    command,
                    C::ReplaceNode { .. }
                        | C::ReplaceSubtree { .. }
                        | C::InsertSubtree { .. }
                        | C::BatchInsert { .. }
                        | C::InsertAuthoredSubtree { .. }
                );
            }
            C::DeleteNode { .. }
            | C::DeleteSelected
            | C::CutSelected
            | C::DeletePage { .. }
            | C::UngroupSelected => self.removes = true,
            _ if !command.may_change_document_identities() => (),
            // Unknown structural/rich patches, history exchange and broad
            // generators do not expose a proved numeric mutation summary.
            _ => self.valid = false,
        }
    }
}
impl EditorState {
    /// Call before any unobserved callback, direct public Doc edit, repair or
    /// rollback. The next trusted stage then re-reads the complete real Doc.
    pub fn invalidate_command_id_sequence(&mut self) {
        self.command_id_seed.valid = false;
    }
    pub(crate) fn observe_command_id_sequence(&mut self, command: &C) {
        self.command_id_seed.observe_lookup(self, command);
        self.command_id_seed.observe(command);
        if self.command_id_seed.entered
            && self.command_id_seed.valid
            && self.command_id_seed.removes
        {
            // Do this BEFORE a deletion/replacement, not merely at the next
            // stage: seed reads after the command inside this same callback
            // must also match a fresh standalone query. Public pre-scope ids
            // are not automatically claimed by the runtime high-water.
            let live = self.command_id_seed.get(self).unwrap();
            let covered = match (live, self.runtime_next_id_seed()) {
                (_, Err(_)) => true,
                (Ok(live), Ok(runtime)) => runtime >= live,
                (Err(_), Ok(_)) => false,
            };
            if !covered {
                self.invalidate_command_id_sequence();
            }
        }
    }
}
