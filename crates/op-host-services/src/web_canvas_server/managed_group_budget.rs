//! Bounded whole-cohort admission. Metadata graphs are charged once and
//! unchanged component owners remain shared across audited command snapshots.
use jian_ops_schema::node::PenNode;
use op_editor_core::history_snapshot::{
    command_capture_node_bytes, CaptureSizeError, COMMAND_CAPTURE_ALLOWANCE,
};
use op_editor_core::{EditorCommand as C, EditorState, NodeId, PenNodeExt};
use std::mem::size_of;
#[path = "managed_group_retention.rs"]
mod retention;
const LIMIT: usize = COMMAND_CAPTURE_ALLOWANCE;
#[derive(Default, Clone, Copy)]
pub(super) struct Forecast {
    pub component_count: usize,
    pub components: usize,
    pub target: usize,
    pub auxiliary: usize,
    pub retained: usize,
    pub projected: usize,
    pub error: usize,
}
#[derive(Default, Clone, Copy)]
pub(super) struct Budget {
    used: usize,
    components: Option<usize>,
    retained: Option<usize>,
    shared_charged: bool,
    last: Forecast,
}
#[derive(Clone, Copy)]
#[repr(usize)]
pub(super) enum Refusal {
    Workspace,
    Redo,
    DocumentState,
    StateOwners,
    LegacyComponents,
    ComponentMetadata,
    Target,
    TargetMetadata,
    Auxiliary,
    EvictedHistory,
    CohortCapacity,
}
impl Refusal {
    pub(super) const ALL: [Self; 11] = [
        Self::Workspace,
        Self::Redo,
        Self::DocumentState,
        Self::StateOwners,
        Self::LegacyComponents,
        Self::ComponentMetadata,
        Self::Target,
        Self::TargetMetadata,
        Self::Auxiliary,
        Self::EvictedHistory,
        Self::CohortCapacity,
    ];
    pub(super) fn code(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Redo => "redo",
            Self::DocumentState => "document-state",
            Self::StateOwners => "state-owners",
            Self::LegacyComponents => "legacy-components",
            Self::ComponentMetadata => "component-metadata",
            Self::Target => "target",
            Self::TargetMetadata => "target-metadata",
            Self::Auxiliary => "auxiliary",
            Self::EvictedHistory => "evicted-history",
            Self::CohortCapacity => "cohort-capacity",
        }
    }
}
fn component_charge(state: &EditorState, limit: usize) -> Result<usize, CaptureSizeError> {
    let mut bytes = state.components.components.len() * (size_of::<usize>() * 4 + 256);
    for component in &state.components.components {
        bytes = bytes.saturating_add(8 * (component.id.as_str().len() + component.name.len()));
        let mut pending = vec![&component.root];
        while let Some(node) = pending.pop() {
            let header = command_capture_node_bytes(node, true, limit.saturating_sub(bytes))
                .map_err(|error| match error {
                    CaptureSizeError::Capacity { attempted, .. } => CaptureSizeError::Capacity {
                        attempted: bytes.saturating_add(attempted),
                        limit,
                    },
                    error => error,
                })?;
            bytes = bytes.saturating_add(header);
            if let Some(children) = node.children() {
                bytes = bytes.saturating_add(
                    children.len() * (size_of::<PenNode>() + size_of::<usize>() * 4),
                );
                if bytes > limit {
                    return Err(CaptureSizeError::Capacity {
                        attempted: bytes,
                        limit,
                    });
                }
                pending.extend(children);
            }
        }
    }
    if bytes > limit {
        Err(CaptureSizeError::Capacity {
            attempted: bytes,
            limit,
        })
    } else {
        Ok(bytes)
    }
}
fn auxiliary_bytes(state: &EditorState) -> usize {
    let mut bytes = 16 * 1024
        + state.ui.variables.active_theme.len() * 1024
        + state
            .ui
            .variables
            .active_theme
            .iter()
            .map(|(key, value)| 8 * (key.len() + value.len()))
            .sum::<usize>()
        + state.app_state_owner.len() * 512
        + state
            .app_state_owner
            .keys()
            .map(|key| 8 * key.len())
            .sum::<usize>();
    for refs in [
        &state.ui.variables.fill_refs,
        &state.ui.variables.stroke_refs,
    ] {
        bytes = bytes
            .saturating_add(refs.capacity() * (size_of::<(NodeId, String)>() + 32) * 2)
            .saturating_add(
                refs.iter()
                    .map(|(key, value)| 8 * (key.as_str().len() + value.len()))
                    .sum::<usize>(),
            );
    }
    bytes = bytes
        .saturating_add(state.selection.set.len() * (size_of::<NodeId>() + 64))
        .saturating_add(
            state
                .selection
                .set
                .iter()
                .map(|id| 8 * id.as_str().len())
                .sum::<usize>(),
        )
        .saturating_add(8 * state.selection.anchor.as_str().len());
    bytes.saturating_add(retention::clone_bytes(state))
}
fn target<'a>(state: &'a EditorState, command: &C) -> Option<&'a PenNode> {
    let (id, page) = match command {
        C::UpdateNode {
            node_id, page_id, ..
        }
        | C::PatchNodeData {
            node_id, page_id, ..
        } => (node_id, page_id.as_deref()),
        C::SetNodeRotation { node_id, .. }
        | C::SetNodeText { node_id, .. }
        | C::SetNodeCornerRadius { node_id, .. }
        | C::SetNodeFontSize { node_id, .. }
        | C::SetNodeFontWeight { node_id, .. }
        | C::SetNodeStrokeHex { node_id, .. }
        | C::SetNodeStrokeWidth { node_id, .. }
        | C::SetNodeFillHex { node_id, .. }
        | C::SetNodeName { node_id, .. }
        | C::SetNodeFlag { node_id, .. }
        | C::SetNodeFlip { node_id, .. }
        | C::SetEllipseArc { node_id, .. }
        | C::AddNodeEffect { node_id, .. }
        | C::RemoveNodeEffect { node_id, .. }
        | C::SetNodeStrokeSideWidth { node_id, .. } => (node_id, None),
        _ => return None,
    };
    let pages = state.doc.pages.as_ref().filter(|pages| !pages.is_empty());
    let raw = page.map(str::trim).filter(|page| !page.is_empty());
    let nodes = match pages {
        None if raw.is_none_or(|page| page == "0") => &state.doc.children,
        None => return None,
        Some(pages) => {
            let index = match raw {
                None => state.ui.active_page_index,
                Some(raw) => pages
                    .iter()
                    .position(|page| page.id == raw)
                    .or_else(|| raw.parse().ok())?,
            };
            &pages.get(index)?.children
        }
    };
    let node = op_editor_core::walkers::find_node(nodes, id)?;
    if matches!(node, PenNode::Ref(_)) {
        return None;
    }
    Some(node)
}

impl Budget {
    pub(super) fn forecast(&self) -> Forecast {
        self.last
    }
    fn size_error(&mut self, error: CaptureSizeError) -> usize {
        match error {
            CaptureSizeError::Capacity { attempted, .. } => {
                self.last.error = 1;
                attempted
            }
            CaptureSizeError::Serialization => {
                self.last.error = 2;
                0
            }
        }
    }
    pub(super) fn retain_unanchored_components(&mut self) {
        // A successful command that did not advance revision adds no history
        // anchor. Its pending capture may therefore retain another component
        // owner; this headroom was reserved before that capture was created.
        self.used = self.used.saturating_add(self.components.unwrap_or(0));
        debug_assert!(self.used <= LIMIT);
    }
    pub(super) fn reserve_live_selection(&mut self, bytes: usize) -> Result<(), Refusal> {
        if bytes > LIMIT.saturating_sub(self.used) {
            return Err(Refusal::CohortCapacity);
        }
        self.used += bytes;
        Ok(())
    }
    pub(super) fn reserve(
        &mut self,
        state: &EditorState,
        command: &C,
        input: usize,
    ) -> Result<(), Refusal> {
        self.last = Forecast {
            component_count: state.components.len(),
            ..Forecast::default()
        };
        if state.editor_ui.workspace.active {
            return Err(Refusal::Workspace);
        }
        let target = if matches!(
            command,
            C::InsertNode { .. } | C::DeleteNode { .. } | C::CopyNode { .. }
        ) {
            state
                .forecast_structural_command_edit(command)
                .ok_or(Refusal::Target)?
        } else {
            let node = target(state, command).ok_or(Refusal::Target)?;
            match command_capture_node_bytes(node, true, LIMIT) {
                Ok(bytes) => bytes,
                Err(error) => {
                    self.last.target = self.size_error(error);
                    return Err(Refusal::TargetMetadata);
                }
            }
        };
        self.last.target = target;
        let components = match self.components {
            Some(bytes) => bytes,
            None => match component_charge(state, LIMIT) {
                Ok(bytes) => {
                    self.components = Some(bytes);
                    bytes
                }
                Err(error) => {
                    self.last.components = self.size_error(error);
                    return Err(Refusal::ComponentMetadata);
                }
            },
        };
        self.last.components = components;
        let retained = match self.retained {
            Some(bytes) => bytes,
            None => match retention::retained_bytes(state, LIMIT.saturating_sub(components)) {
                Ok(bytes) => {
                    self.retained = Some(bytes);
                    bytes
                }
                Err(error) => {
                    self.last.retained = self.size_error(error);
                    return Err(Refusal::EvictedHistory);
                }
            },
        };
        self.last.retained = retained;
        let auxiliary = auxiliary_bytes(state);
        self.last.auxiliary = auxiliary;
        let shared = if self.shared_charged {
            0
        } else {
            components.saturating_add(retained)
        };
        let charge = target
            .saturating_add(input.saturating_mul(8))
            .saturating_add(auxiliary)
            .saturating_add(shared);
        // Reserve one transient component capture/materialization peak, without
        // charging that unchanged immutable owner once for every writer.
        let peak = charge.saturating_add(components);
        self.last.projected = self.used.saturating_add(peak);
        if peak > LIMIT.saturating_sub(self.used) {
            return Err(Refusal::CohortCapacity);
        }
        self.used += charge;
        self.shared_charged = true;
        Ok(())
    }
}
