//! Local computed islands inside an otherwise authored-geometry document.
use super::*;
use jian_ops_schema::{
    node::container::ContainerProps,
    sizing::{SizingBehavior, SizingKeyword},
};
use op_editor_core::pen_node_ext::{clone_without_children, PenNodeExt};

fn container(node: &PenNode) -> Option<&ContainerProps> {
    match node {
        PenNode::Frame(n) => Some(&n.container),
        PenNode::Group(n) => Some(&n.container),
        PenNode::Rectangle(n) => Some(&n.container),
        _ => None,
    }
}
fn container_mut(node: &mut PenNode) -> Option<&mut ContainerProps> {
    match node {
        PenNode::Frame(n) => Some(&mut n.container),
        PenNode::Group(n) => Some(&mut n.container),
        PenNode::Rectangle(n) => Some(&mut n.container),
        _ => None,
    }
}
pub(super) fn has_override(node: &PenNode) -> bool {
    container(node).is_some_and(|c| c.preserve_geometry == Some(false))
}

pub(super) fn apply(nodes: &[&PenNode], out: &mut BTreeMap<String, [f32; 4]>) {
    // The authored rectangle traversal collects only explicit overrides in
    // postorder. Imported files with no edits allocate/solve nothing extra.
    // Intrinsic fit sizes go bottom-up; allocated fill sizes then go top-down.
    for node in nodes {
        reflow(node, out, false);
    }
    for node in nodes.iter().rev() {
        reflow(node, out, true);
    }
}
fn fill(value: &Option<SizingBehavior>) -> bool {
    matches!(
        value,
        Some(SizingBehavior::Keyword(SizingKeyword::FillContainer))
    )
}
fn freeze(value: &mut Option<SizingBehavior>, size: f32) {
    if !fill(value) && size.is_finite() && size >= 0.0 {
        *value = Some(SizingBehavior::Number(f64::from(size)));
    }
}
fn reflow(node: &PenNode, out: &mut BTreeMap<String, [f32; 4]>, allocated: bool) {
    let Some(old) = out.get(node.id_str()).copied() else {
        return;
    };
    let mut shallow = clone_without_children(node);
    if allocated {
        let c = container_mut(&mut shallow).expect("opted-in container");
        if fill(&c.width) && old[2] > 0.0 {
            c.width = Some(SizingBehavior::Number(f64::from(old[2])));
        }
        if fill(&c.height) && old[3] > 0.0 {
            c.height = Some(SizingBehavior::Number(f64::from(old[3])));
        }
    }
    for child in children_of(node) {
        let mut metadata = clone_without_children(child);
        if let (Some(c), Some(rect)) = (container_mut(&mut metadata), out.get(child.id_str())) {
            // A preserved nested frame is an intrinsic box to this parent,
            // not permission to reflow its imported grandchildren.
            freeze(&mut c.width, rect[2]);
            freeze(&mut c.height, rect[3]);
            c.layout = Some(jian_ops_schema::node::container::LayoutMode::None);
            // Its existing padding is internal to the preserved box. Leaving
            // it on an empty proxy could expand a60px imported box to its
            // padding sum and silently rewrite the parent flow.
            c.padding = None;
            c.gap = None;
            if !has_override(child) {
                c.limits = Default::default();
            }
        }
        shallow
            .children_mut()
            .expect("opted-in container")
            .push(metadata);
    }
    let mut solved = BTreeMap::new();
    crate::adapter::pages::compute_layout_with_measure(
        &shallow,
        &mut solved,
        crate::adapter::pages::layout_measure_backend(),
    );
    let Some(root) = solved.get(node.id_str()).copied() else {
        return;
    };
    let offset = [old[0] - root[0], old[1] - root[1]];
    out.insert(node.id_str().to_owned(), [old[0], old[1], root[2], root[3]]);
    for child in children_of(node) {
        let (Some(previous), Some(next)) =
            (out.get(child.id_str()).copied(), solved.get(child.id_str()))
        else {
            continue;
        };
        let next = [next[0] + offset[0], next[1] + offset[1], next[2], next[3]];
        shift(child, next[0] - previous[0], next[1] - previous[1], out);
        out.insert(child.id_str().to_owned(), next);
    }
}
fn shift(node: &PenNode, dx: f32, dy: f32, out: &mut BTreeMap<String, [f32; 4]>) {
    if let Some(rect) = out.get_mut(node.id_str()) {
        rect[0] += dx;
        rect[1] += dy;
    }
    for child in children_of(node) {
        shift(child, dx, dy, out);
    }
}
