//! Private state and allocation helpers for one drawing program.

use super::batch_program::Result;
use super::batch_program_error::ProgramError;
use super::batch_program_node_parse::NodeParseOptions;
use super::EditorCommand;
use jian_ops_schema::node::PenNode;
use op_editor_core::command_node::remap_subtree_ids_mapping;
use op_editor_core::{CommandSimulation, IdAllocator, SequentialIdAllocator};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct ProgramCtx {
    pub(crate) sim: CommandSimulation,
    pub(crate) page_id: Option<String>,
    /// binding name → final (host-assigned) node id.
    pub(crate) bindings: BTreeMap<String, String>,
    /// authored id → final id for remapped inserts, first-wins (TS
    /// `findNodeInTree` finds the first match in tree order).
    pub(crate) alias: BTreeMap<String, String>,
    pub(crate) results: Vec<Value>,
    pub(crate) commands: Vec<EditorCommand>,
    pub(crate) post_process: bool,
    /// Monotonic counter for `_auto_*` bindless-line bindings.
    pub(crate) auto_seq: usize,
    /// Index of the operation currently being executed.
    pub(crate) current_line: usize,
    /// Append G() lines whose result binding receives explicit positive
    /// numeric width and height later in this same program.
    pub(crate) explicitly_sized_append_lines: BTreeSet<usize>,
    /// Root placeholders that existed before this program started. Consumed
    /// at most once so newly inserted empty screen shells remain siblings.
    pub(crate) replaceable_empty_root_ids: Vec<String>,
    /// Highest id this program has handed out, so an id freed by a `D()`
    /// earlier in the SAME batch is never reissued. The document's own seed
    /// is `max(existing) + 1`, which walks backwards after a delete: a
    /// measured batch bound both the deleted badge and the image that
    /// replaced it to `n16`, leaving one binding pointing at a node the
    /// caller believes is gone.
    pub(crate) id_high_water: u64,
    /// Non-fatal notes for the line being executed; drained into the
    /// envelope's `warnings[]` after each line.
    pub(crate) warnings: Vec<String>,
    /// The orchestrator's best-effort policy: node bodies may additionally
    /// be salvaged by dropping one field the schema rejects.
    pub(crate) best_effort: bool,
}

impl ProgramCtx {
    /// Emit `cmd` AND apply it to the sim. The sim apply is the line's
    /// final validation gate — the host will run the same code.
    pub(crate) fn emit(&mut self, cmd: EditorCommand, failure: &str) -> Result<()> {
        if !self.sim.apply(cmd.clone()) {
            return Err(ProgramError::ApplyRejected(failure.to_string()));
        }
        self.commands.push(cmd);
        Ok(())
    }

    pub(crate) fn warn(&mut self, warning: String) {
        self.warnings.push(warning);
    }

    /// Parse options for a node body under this program's policy.
    pub(crate) fn node_parse_options(&self, document_root: bool) -> NodeParseOptions {
        NodeParseOptions {
            post_process: self.post_process,
            document_root,
            handle_refs: false,
            salvage_fields: self.best_effort,
        }
    }

    pub(crate) fn bind(&mut self, binding: &str, node_id: &str) {
        self.bindings
            .insert(binding.to_string(), node_id.to_string());
        // A re-bound name (a redraft, or scratch reuse) updates its existing
        // results entry in place — consumers look bindings up by FIRST match,
        // which must never point at a superseded draft's deleted id.
        if let Some(entry) = self
            .results
            .iter_mut()
            .find(|r| r.get("binding").and_then(Value::as_str) == Some(binding))
        {
            entry["nodeId"] = json!(node_id);
        } else {
            self.results
                .push(json!({ "binding": binding, "nodeId": node_id }));
        }
    }

    /// Assign fresh sim-allocator ids to `nodes` (in place); returns
    /// the (authored → final) map. Authored ids are recorded into the
    /// alias table so slash paths keep resolving TS-style.
    pub(crate) fn remap(&mut self, nodes: &mut [PenNode]) -> Result<Vec<(String, String)>> {
        let map = self.remap_copied_nodes(nodes)?;
        for (old, new) in &map {
            if !old.starts_with("__op_tmp_") {
                self.alias.entry(old.clone()).or_insert_with(|| new.clone());
            }
        }
        Ok(map)
    }

    /// Predict the same remap for a replacement without cloning its payload.
    /// Use the canonical allocator and commit aliases/high-water only after the
    /// complete allocation succeeds, exactly as the mutating remap does.
    pub(crate) fn preview_remap(&mut self, nodes: &[PenNode]) -> Result<Vec<(String, String)>> {
        let seed = self
            .sim
            .next_program_id_seed(self.id_high_water)
            .ok_or(ProgramError::IdSpaceExhausted)?
            .max(self.id_high_water);
        let mut allocator = SequentialIdAllocator::new(seed);
        let mut taken = std::collections::HashSet::new();
        let mut map = Vec::new();
        for entry in crate::tools::read_walk::nodes(nodes) {
            let new = allocator
                .allocate(&mut taken)
                .map_err(|_| ProgramError::IdSpaceExhausted)?;
            map.push((entry.node.id_str().to_owned(), String::from(new)));
        }
        self.id_high_water = allocator.next_counter();
        for (old, new) in &map {
            if !old.starts_with("__op_tmp_") {
                self.alias.entry(old.clone()).or_insert_with(|| new.clone());
            }
        }
        Ok(map)
    }

    /// Copies start from live source ids. Allocating new ids must not redirect
    /// later references to that source (or its children) into the copy.
    pub(crate) fn remap_copied_nodes(
        &mut self,
        nodes: &mut [PenNode],
    ) -> Result<Vec<(String, String)>> {
        let mut seed = self
            .sim
            .next_program_id_seed(self.id_high_water)
            .ok_or(ProgramError::IdSpaceExhausted)?
            .max(self.id_high_water);
        // `seed` is above every current node/page numeric id, and remapping
        // emits only fresh n{counter} ids. Reserve just this incoming forest.
        let mut taken = std::collections::HashSet::new();
        let map = remap_subtree_ids_mapping(nodes, &mut seed, &mut taken)
            .ok_or(ProgramError::IdSpaceExhausted)?;
        self.id_high_water = seed;
        Ok(map)
    }
}
