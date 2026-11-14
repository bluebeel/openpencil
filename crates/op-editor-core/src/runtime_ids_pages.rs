//! Direct allocator-aware entry points share the runtime ownership observer.
use crate::{EditorState, IdAllocError, IdAllocator, NodeId};
use jian_ops_schema::node::PenNode;
impl EditorState {
    #[allow(clippy::too_many_arguments)]
    pub fn set_active_page_background_color_with_allocator(
        &mut self,
        color: Option<String>,
        allocator: &mut dyn IdAllocator,
    ) -> Result<bool, IdAllocError> {
        self.observe_runtime_allocations(
            allocator,
            |state, observed| {
                state.set_active_page_background_color_with_allocator_unobserved(color, observed)
            },
            |result| *result,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn add_page_with_allocator(
        &mut self,
        name: Option<String>,
        children: Option<Vec<PenNode>>,
        allocator: &mut dyn IdAllocator,
    ) -> Result<Option<usize>, IdAllocError> {
        self.observe_runtime_allocations(
            allocator,
            |state, observed| state.add_page_with_allocator_unobserved(name, children, observed),
            |result| result.is_some(),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn append_components_page_masters_with_allocator(
        &mut self,
        masters: Vec<PenNode>,
        allocator: &mut dyn IdAllocator,
    ) -> Result<usize, IdAllocError> {
        self.observe_runtime_allocations(
            allocator,
            |state, observed| {
                state.append_components_page_masters_with_allocator_unobserved(masters, observed)
            },
            |result| *result > 0,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn duplicate_page_with_allocator(
        &mut self,
        idx: usize,
        name: Option<String>,
        allocator: &mut dyn IdAllocator,
    ) -> Result<Option<usize>, IdAllocError> {
        self.observe_runtime_allocations(
            allocator,
            |state, observed| state.duplicate_page_with_allocator_unobserved(idx, name, observed),
            |result| result.is_some(),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn instantiate_kit_component_under_parent_with_allocator(
        &mut self,
        kit_id: &str,
        component_id: &str,
        parent_id: &NodeId,
        doc_x: f64,
        doc_y: f64,
        overrides_json: Option<&str>,
        allocator: &mut dyn IdAllocator,
    ) -> Result<Option<NodeId>, IdAllocError> {
        self.observe_runtime_allocations(
            allocator,
            |state, observed| {
                state.instantiate_kit_component_under_parent_with_allocator_unobserved(
                    kit_id,
                    component_id,
                    parent_id,
                    doc_x,
                    doc_y,
                    overrides_json,
                    observed,
                )
            },
            |result| result.is_some(),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn import_svg_with_allocator(
        &mut self,
        allocator: &mut dyn IdAllocator,
        svg: &str,
        offset: (f64, f64),
    ) -> Result<usize, IdAllocError> {
        self.observe_runtime_allocations(
            allocator,
            |state, observed| state.import_svg_with_allocator_unobserved(observed, svg, offset),
            |result| *result > 0,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn import_svg_named_with_allocator(
        &mut self,
        allocator: &mut dyn IdAllocator,
        svg: &str,
        offset: (f64, f64),
        group_name: Option<&str>,
    ) -> Result<usize, IdAllocError> {
        self.observe_runtime_allocations(
            allocator,
            |state, observed| {
                state.import_svg_named_with_allocator_unobserved(observed, svg, offset, group_name)
            },
            |result| *result > 0,
        )
    }
}
