//! Widget-prop writers for the per-node attribute commands: the
//! string-typed (`placeholder` / `value` / `label` / leading + trailing
//! icon), numeric (`min` / `max` / `step` / `value`), and `bindings`
//! slots on the form-widget variants. Carved off `command_node_attrs.rs`
//! to keep every file under the 800-line cap.

use super::{WidgetNumberField, WidgetTextField};
use jian_ops_schema::node::{NumberOrExpression, PenNode};

/// Write a string-typed widget prop. Returns true only when the
/// node variant actually carries `field`.
pub(super) fn write_widget_text(node: &mut PenNode, field: WidgetTextField, text: &str) -> bool {
    use WidgetTextField as F;
    let slot = match (node, field) {
        (PenNode::TextInput(n), F::Placeholder) => &mut n.placeholder,
        (PenNode::TextInput(n), F::Value) => &mut n.value,
        (PenNode::TextArea(n), F::Placeholder) => &mut n.placeholder,
        (PenNode::TextArea(n), F::Value) => &mut n.value,
        (PenNode::NumberInput(n), F::Placeholder) => &mut n.placeholder,
        (PenNode::Select(n), F::Placeholder) => &mut n.placeholder,
        (PenNode::Select(n), F::Value) => &mut n.value,
        (PenNode::RadioGroup(n), F::Value) => &mut n.value,
        (PenNode::Checkbox(n), F::Label) => &mut n.label,
        (PenNode::TextInput(n), F::LeadingIcon) => &mut n.leading_icon,
        (PenNode::TextInput(n), F::TrailingIcon) => &mut n.trailing_icon,
        (PenNode::TextArea(n), F::LeadingIcon) => &mut n.leading_icon,
        (PenNode::TextArea(n), F::TrailingIcon) => &mut n.trailing_icon,
        (PenNode::NumberInput(n), F::LeadingIcon) => &mut n.leading_icon,
        (PenNode::NumberInput(n), F::TrailingIcon) => &mut n.trailing_icon,
        _ => return false,
    };
    // Do not allocate a string before the actual typed target accepts it.
    *slot = (!text.is_empty()).then(|| text.to_owned());
    true
}

/// Mutably borrow whatever variant's `bindings` map. `None` for kinds
/// that don't carry one (Frame / Group / Rectangle / Text / Ellipse).
pub(super) fn node_bindings_slot(
    node: &mut PenNode,
) -> Option<&mut Option<jian_ops_schema::events::Bindings>> {
    match node {
        PenNode::TextInput(n) => Some(&mut n.bindings),
        PenNode::TextArea(n) => Some(&mut n.bindings),
        PenNode::NumberInput(n) => Some(&mut n.bindings),
        PenNode::Select(n) => Some(&mut n.bindings),
        PenNode::RadioGroup(n) => Some(&mut n.bindings),
        PenNode::Switch(n) => Some(&mut n.bindings),
        PenNode::Checkbox(n) => Some(&mut n.bindings),
        PenNode::Tabs(n) => Some(&mut n.bindings),
        _ => None,
    }
}

/// Write a numeric widget prop. `min` / `max` / `step` are plain
/// `f64`; numeric `value` writes a literal `NumberOrExpression`,
/// overwriting any prior expression binding. Returns true only when
/// the node variant carries `field`.
pub(super) fn write_widget_number(
    node: &mut PenNode,
    field: WidgetNumberField,
    value: f64,
) -> bool {
    use WidgetNumberField as F;
    match (node, field) {
        (PenNode::Slider(n), F::Min) => n.min = Some(value),
        (PenNode::Slider(n), F::Max) => n.max = Some(value),
        (PenNode::Slider(n), F::Step) => n.step = Some(value),
        (PenNode::Slider(n), F::Value) => n.value = Some(NumberOrExpression::Number(value)),
        (PenNode::NumberInput(n), F::Min) => n.min = Some(value),
        (PenNode::NumberInput(n), F::Max) => n.max = Some(value),
        (PenNode::NumberInput(n), F::Step) => n.step = Some(value),
        (PenNode::NumberInput(n), F::Value) => n.value = Some(NumberOrExpression::Number(value)),
        (PenNode::Progress(n), F::Max) => n.max = Some(value),
        (PenNode::Progress(n), F::Value) => n.value = Some(NumberOrExpression::Number(value)),
        _ => return false,
    }
    true
}
