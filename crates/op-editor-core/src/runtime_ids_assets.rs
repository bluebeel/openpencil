//! Direct allocator-aware entry points share the runtime ownership observer.
use crate::{EditorState, IdAllocError, IdAllocator, NodeId, Tool};

impl EditorState {
    #[allow(clippy::too_many_arguments)]
    pub fn create_node_for_tool_with_allocator(
        &mut self,
        tool: Tool,
        allocator: &mut dyn IdAllocator,
        doc_x: f64,
        doc_y: f64,
        init_w: f64,
        init_h: f64,
    ) -> Result<Option<NodeId>, IdAllocError> {
        self.observe_runtime_allocations(
            allocator,
            |state, observed| {
                state.create_node_for_tool_with_allocator_unobserved(
                    tool, observed, doc_x, doc_y, init_w, init_h,
                )
            },
            |result| result.is_some(),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn insert_image_node_at_viewport_with_allocator(
        &mut self,
        name: &str,
        src: &str,
        allocator: &mut dyn IdAllocator,
    ) -> Result<Option<NodeId>, IdAllocError> {
        self.observe_runtime_allocations(
            allocator,
            |state, observed| {
                state.insert_image_node_at_viewport_with_allocator_unobserved(name, src, observed)
            },
            |result| result.is_some(),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn insert_image_node_at_viewport_sized_with_allocator(
        &mut self,
        name: &str,
        src: &str,
        pixel_width: u32,
        pixel_height: u32,
        allocator: &mut dyn IdAllocator,
    ) -> Result<Option<NodeId>, IdAllocError> {
        self.observe_runtime_allocations(
            allocator,
            |state, observed| {
                state.insert_image_node_at_viewport_sized_with_allocator_unobserved(
                    name,
                    src,
                    pixel_width,
                    pixel_height,
                    observed,
                )
            },
            |result| result.is_some(),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn insert_image_node_at_doc_point_sized_with_allocator(
        &mut self,
        name: &str,
        src: &str,
        pixel_width: u32,
        pixel_height: u32,
        centre: (f64, f64),
        allocator: &mut dyn IdAllocator,
    ) -> Result<Option<NodeId>, IdAllocError> {
        self.observe_runtime_allocations(
            allocator,
            |state, observed| {
                state.insert_image_node_at_doc_point_sized_with_allocator_unobserved(
                    name,
                    src,
                    pixel_width,
                    pixel_height,
                    centre,
                    observed,
                )
            },
            |result| result.is_some(),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn insert_icon_font_node_at_with_allocator(
        &mut self,
        icon_name: &str,
        family: &str,
        center_x: f64,
        center_y: f64,
        allocator: &mut dyn IdAllocator,
    ) -> Result<Option<NodeId>, IdAllocError> {
        self.observe_runtime_allocations(
            allocator,
            |state, observed| {
                state.insert_icon_font_node_at_with_allocator_unobserved(
                    icon_name, family, center_x, center_y, observed,
                )
            },
            |result| result.is_some(),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn insert_icon_node_at_with_allocator(
        &mut self,
        icon_name: &str,
        family: &str,
        svg_path_d: Option<&str>,
        center_x: f64,
        center_y: f64,
        allocator: &mut dyn IdAllocator,
    ) -> Result<Option<NodeId>, IdAllocError> {
        self.observe_runtime_allocations(
            allocator,
            |state, observed| {
                state.insert_icon_node_at_with_allocator_unobserved(
                    icon_name, family, svg_path_d, center_x, center_y, observed,
                )
            },
            |result| result.is_some(),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn replace_paths_with_polyline_with_allocator(
        &mut self,
        source_ids: &[NodeId],
        contours: &[Vec<(f64, f64)>],
        allocator: &mut dyn IdAllocator,
    ) -> Result<Option<NodeId>, IdAllocError> {
        self.observe_runtime_allocations(
            allocator,
            |state, observed| {
                state.replace_paths_with_polyline_with_allocator_unobserved(
                    source_ids, contours, observed,
                )
            },
            |result| result.is_some(),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn replace_paths_with_svg_with_allocator(
        &mut self,
        source_ids: &[NodeId],
        d: String,
        bounds: (f64, f64, f64, f64),
        allocator: &mut dyn IdAllocator,
    ) -> Result<Option<NodeId>, IdAllocError> {
        self.observe_runtime_allocations(
            allocator,
            |state, observed| {
                state.replace_paths_with_svg_with_allocator_unobserved(
                    source_ids, d, bounds, observed,
                )
            },
            |result| result.is_some(),
        )
    }
}
