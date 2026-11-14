//! Direct allocator-aware entry points share the runtime ownership observer.
use crate::{EditorState, IdAllocError, IdAllocator, NodeId};
use jian_ops_schema::node::PenNode;
impl EditorState {
    #[allow(clippy::too_many_arguments)]
    pub fn duplicate_selected_with_allocator(
        &mut self,
        allocator: &mut dyn IdAllocator,
        offset_doc_px: f64,
    ) -> Result<Option<NodeId>, IdAllocError> {
        self.observe_runtime_allocations(
            allocator,
            |state, observed| {
                state.duplicate_selected_with_allocator_unobserved(observed, offset_doc_px)
            },
            |result| result.is_some(),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn paste_clipboard_with_allocator(
        &mut self,
        allocator: &mut dyn IdAllocator,
        offset_doc_px: f64,
    ) -> Result<Vec<NodeId>, IdAllocError> {
        self.observe_runtime_allocations(
            allocator,
            |state, observed| {
                state.paste_clipboard_with_allocator_unobserved(observed, offset_doc_px)
            },
            |result| !result.is_empty(),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn group_selected_with_allocator(
        &mut self,
        allocator: &mut dyn IdAllocator,
    ) -> Result<Option<NodeId>, IdAllocError> {
        self.observe_runtime_allocations(
            allocator,
            |state, observed| state.group_selected_with_allocator_unobserved(observed),
            |result| result.is_some(),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn start_pen_path_with_allocator(
        &mut self,
        allocator: &mut dyn IdAllocator,
        first: (f64, f64),
    ) -> Result<Option<NodeId>, IdAllocError> {
        self.observe_runtime_allocations(
            allocator,
            |state, observed| state.start_pen_path_with_allocator_unobserved(observed, first),
            |result| result.is_some(),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn instantiate_component_with_allocator(
        &mut self,
        component_id: &NodeId,
        allocator: &mut dyn IdAllocator,
    ) -> Result<Option<NodeId>, IdAllocError> {
        self.observe_runtime_allocations(
            allocator,
            |state, observed| {
                state.instantiate_component_with_allocator_unobserved(component_id, observed)
            },
            |result| result.is_some(),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn detach_component_with_allocator(
        &mut self,
        node_id: &NodeId,
        allocator: &mut dyn IdAllocator,
    ) -> Result<Option<NodeId>, IdAllocError> {
        self.observe_runtime_allocations(
            allocator,
            |state, observed| state.detach_component_with_allocator_unobserved(node_id, observed),
            |result| result.is_some(),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn insert_subtree_returning_root_ids_with_allocator(
        &mut self,
        nodes: Vec<PenNode>,
        parent_id: &NodeId,
        allocator: &mut dyn IdAllocator,
    ) -> Result<Option<Vec<String>>, IdAllocError> {
        self.observe_runtime_allocations(
            allocator,
            |state, observed| {
                state.insert_subtree_returning_root_ids_with_allocator_unobserved(
                    nodes, parent_id, observed,
                )
            },
            |result| result.is_some(),
        )
    }
}
