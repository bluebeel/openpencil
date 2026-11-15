//! SVG viewport clipping uses existing canonical vector mask semantics.
use super::*;
use jian_ops_schema::node::MaskType;
use kurbo::Affine;
pub(super) fn clip_viewport(
    mut node: PenNode,
    attrs: &[(String, String)],
    matrix: Affine,
    root: bool,
    allocator: &mut dyn crate::IdAllocator,
    taken: &mut std::collections::HashSet<NodeId>,
) -> Result<PenNode, IdAllocError> {
    if extract_style_or_attr(attrs, "overflow").as_deref() == Some("visible") {
        return Ok(node);
    }
    let vb = super::transform::viewbox(attrs).ok().flatten();
    let w =
        super::transform::length(attrs, "width", vb.map(|v| v[2]).unwrap_or(100.0)).unwrap_or(0.0);
    let h =
        super::transform::length(attrs, "height", vb.map(|v| v[3]).unwrap_or(100.0)).unwrap_or(0.0);

    // matrix already contains the viewBox mapping. Undo only that factor,
    // retaining the authored transform, nested origin, root fit and offset.
    let view = super::transform::viewbox_matrix(attrs, vb, w, h).unwrap_or(Affine::IDENTITY);
    let frame = matrix * view.inverse();
    let empty = w <= 0.0 || h <= 0.0;
    let d = if empty {
        "M0 0 H1 V1 H0 Z".into()
    } else {
        format!("M0 0 H{w} V{h} H0 Z")
    };
    let id = allocator.allocate(taken)?;
    if let Some(mut source) = path_node_from_svg_d(
        id,
        if root {
            "SVG root viewport"
        } else {
            "SVG viewport"
        },
        &d,
        frame,
        Some(vec![solid_fill(if empty { "#000000" } else { "#ffffff" })]),
        None,
        Some(PathFillRule::Nonzero),
    ) {
        source.base_mut().mask_type = Some(if empty {
            MaskType::Luminance
        } else {
            MaskType::Vector
        });
        let id = allocator.allocate(taken)?;
        node = import_group(id, "SVG viewport", vec![node, source]);
    }
    Ok(node)
}
