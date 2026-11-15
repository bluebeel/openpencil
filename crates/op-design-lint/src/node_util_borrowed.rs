//! Read wire-level scalar fields without serializing descendant trees.
use super::{base, PenNode, SizingBehavior};
/// Exactly JSON `.get(width/height).as_f64()`: keyword/expression/absent and
/// nonfinite values produce None; valid zero/negative/signed-zero are retained.
pub fn numeric_wire_size(node: &PenNode, width: bool) -> Option<f64> {
    match sizing(node, width) {
        Some(SizingBehavior::Number(value)) if value.is_finite() => Some(*value),
        _ => None,
    }
}
/// One exhaustive canonical dimension accessor, shared by wire and lint views.
pub(super) fn sizing(node: &PenNode, width: bool) -> Option<&SizingBehavior> {
    macro_rules! size {
        ($node:expr) => {
            if width {
                ($node).width.as_ref()
            } else {
                ($node).height.as_ref()
            }
        };
    }
    match node {
        PenNode::Frame(node) => size!(node.container),
        PenNode::Group(node) => size!(node.container),
        PenNode::Rectangle(node) => size!(node.container),
        PenNode::Ellipse(node) => size!(node),
        PenNode::Polygon(node) => size!(node),
        PenNode::Path(node) => size!(node),
        PenNode::Text(node) => size!(node),
        PenNode::Image(node) => size!(node),
        PenNode::IconFont(node) => size!(node),
        PenNode::TextInput(node) => size!(node),
        PenNode::TextArea(node) => size!(node),
        PenNode::Select(node) => size!(node),
        PenNode::Switch(node) => size!(node),
        PenNode::Checkbox(node) => size!(node),
        PenNode::Slider(node) => size!(node),
        PenNode::RadioGroup(node) => size!(node),
        PenNode::NumberInput(node) => size!(node),
        PenNode::Progress(node) => size!(node),
        PenNode::Tabs(node) => size!(node),
        PenNode::Line(_) | PenNode::Ref(_) => None,
    }
}
pub fn identity_label(node: &PenNode) -> String {
    let base = base(node);
    let mut label = base.id.to_ascii_lowercase();
    for text in [base.role.as_deref(), base.name.as_deref()]
        .into_iter()
        .flatten()
    {
        label.push(' ');
        let start = label.len();
        label.push_str(text);
        label[start..].make_ascii_lowercase();
    }
    label
}
