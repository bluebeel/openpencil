//! Borrowed geometry writer shared by standalone UpdateNode and fused runs.
use crate::fills::set_primary_fill_hex;
use crate::{NodeId, PenNodeExt};
use jian_ops_schema::node::PenNode;

#[allow(clippy::too_many_arguments)]
pub(crate) fn update_node(
    node: &mut PenNode,
    node_id: &NodeId,
    x: Option<i32>,
    y: Option<i32>,
    width: Option<i32>,
    height: Option<i32>,
    name: &Option<String>,
    fill_hex: &Option<String>,
) -> bool {
    if !node_id.is_real() {
        return false;
    }
    // Pre-validate EVERY field before the mutable borrow + writes.
    if let Some(w) = width {
        if w < 0 {
            return false;
        }
    }
    if let Some(h) = height {
        if h < 0 {
            return false;
        }
    }
    if let Some(hex) = fill_hex {
        if crate::color_picker::parse_hex_rgb(hex).is_none() {
            return false;
        }
    }
    // All validation passed — every field applies atomically.
    if let Some(nx) = x {
        node.base_mut().x = Some(nx as f64);
    }
    if let Some(ny) = y {
        node.base_mut().y = Some(ny as f64);
    }
    if let Some(nw) = width {
        node.set_width_px(nw as f64);
    }
    if let Some(nh) = height {
        node.set_height_px(nh as f64);
    }
    if let Some(new_name) = name {
        node.base_mut().name = Some(new_name.clone());
    }
    if let Some(hex) = fill_hex {
        set_primary_fill_hex(node, hex);
    }
    true
}
