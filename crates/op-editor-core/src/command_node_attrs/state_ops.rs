//! `impl EditorState` for the per-node attribute commands —
//! `SetNodeRotation` / `SetNodeText` / `SetNodeCornerRadius[At]` /
//! `SetPolygonCount` / `SetNodeFontSize` / `SetNodeFontWeight` /
//! `SetNodeStroke*` / `SetNodeFillHex` / `SetNodeName` / `SetNodeFlag` /
//! `SetNodeFlip` / `SetEllipseArc` / the effect family / the widget-prop
//! family.
//!
//! Each method keeps the validate-then-mutate discipline: kind / range /
//! hex checks happen BEFORE the mutable borrow + write. Carved off
//! `command_node_attrs.rs` to keep every file under the 800-line cap.

use super::slots::{node_effects_slot, write_corner_radius_at};
use super::widget_props::{node_bindings_slot, write_widget_number, write_widget_text};
use super::{WidgetNumberField, WidgetTextField};
use crate::command::{EffectField, NodeFlag, StrokeSide};
use crate::node_id::NodeId;
use crate::pen_node_ext::PenNodeExt;
use crate::state::EditorState;
use crate::walkers::find_node_mut;
use jian_ops_schema::node::{BoolOrExpression, PenNode};
use jian_ops_schema::style::PenEffect;

impl EditorState {
    /// `SetNodeRotation` — write rotation (degrees) on a node.
    pub(crate) fn cmd_set_node_rotation(&mut self, node_id: &NodeId, degrees: f32) -> bool {
        if !node_id.is_real() {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        super::borrowed::set_node_rotation(node, node_id, degrees)
    }

    /// `SetNodeText` — set the plain-text content of a Text node.
    /// Rejects non-Text kinds (parity with shell-core).
    pub(crate) fn cmd_set_node_text(&mut self, node_id: &NodeId, text: &str) -> bool {
        if !node_id.is_real() {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        super::borrowed::set_node_text(node, node_id, text)
    }

    /// `SetNodeCornerRadius` — write corner radius on a node. Rejects
    /// negative / non-finite values.
    pub(crate) fn cmd_set_node_corner_radius(&mut self, node_id: &NodeId, radius: f32) -> bool {
        if !node_id.is_real() {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        super::borrowed::set_node_corner_radius(node, node_id, radius)
    }

    pub(crate) fn cmd_set_node_corner_radius_at(
        &mut self,
        node_id: &NodeId,
        index: usize,
        radius: f32,
    ) -> bool {
        if !node_id.is_real() || !radius.is_finite() || radius < 0.0 {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        write_corner_radius_at(node, index, radius as f64)
    }

    /// Set the side count on a Polygon node. The TS panel allows
    /// `3..=100`; keep the same guard here so all callers converge.
    pub(crate) fn cmd_set_polygon_count(&mut self, node_id: &NodeId, count: u32) -> bool {
        if !node_id.is_real() || !self.is_editable(node_id) {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        match node {
            PenNode::Polygon(p) => {
                p.polygon_count = count.clamp(3, 100);
                true
            }
            _ => false,
        }
    }

    /// `SetNodeFontSize` — set the font size on a Text node. Rejects
    /// non-Text kinds + non-positive sizes.
    pub(crate) fn cmd_set_node_font_size(&mut self, node_id: &NodeId, font_size: f32) -> bool {
        if !node_id.is_real() {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        super::borrowed::set_node_font_size(node, node_id, font_size)
    }

    /// `SetNodeFontWeight` — set the font weight (1..=1000) on a Text
    /// node. Rejects non-Text kinds + out-of-range weights.
    pub(crate) fn cmd_set_node_font_weight(&mut self, node_id: &NodeId, font_weight: u16) -> bool {
        if !node_id.is_real() {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        super::borrowed::set_node_font_weight(node, node_id, font_weight)
    }

    /// `SetNodeStrokeHex` — set the stroke color on a node. A node with
    /// no stroke gets a fresh 1-px stroke so the color always lands.
    ///
    /// Accepts either a literal `#RGB`/`#RRGGBB` hex OR a `$variable-name`
    /// design-token reference. The underlying `PenStroke.fill[*].color`
    /// field is a free `String`; `op-design-lint` detectors emit
    /// `$--border` refs when the doc declares that variable, and a
    /// strict hex parse here would silently drop them (regression caught
    /// by a stop-time review of `LintPreValidator`).
    pub(crate) fn cmd_set_node_stroke_hex(&mut self, node_id: &NodeId, hex: &str) -> bool {
        if !node_id.is_real() {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        super::borrowed::set_node_stroke_hex(node, node_id, hex)
    }

    /// `SetNodeStrokeWidth` — set the stroke width (doc-px) on a node.
    /// Width 0 clears the stroke; width > 0 on a node with no stroke
    /// attaches a fresh stroke at that width.
    pub(crate) fn cmd_set_node_stroke_width(&mut self, node_id: &NodeId, width: f32) -> bool {
        if !node_id.is_real() {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        super::borrowed::set_node_stroke_width(node, node_id, width)
    }

    /// `SetNodeStrokeSideWidth` — set one side's stroke width
    /// (doc-px) on a node. Other sides are preserved; a uniform
    /// stroke expands to explicit top/right/bottom/left widths.
    pub(crate) fn cmd_set_node_stroke_side_width(
        &mut self,
        node_id: &NodeId,
        side: StrokeSide,
        width: f32,
    ) -> bool {
        self.cmd_set_node_stroke_side_width_exact(node_id, side, f64::from(width))
    }

    pub(crate) fn cmd_set_node_stroke_side_width_exact(
        &mut self,
        node_id: &NodeId,
        side: StrokeSide,
        width: f64,
    ) -> bool {
        if !node_id.is_real() {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        super::borrowed::set_node_stroke_side_width_exact(node, node_id, side, width)
    }

    /// `SetNodeFillHex` — set the fill color on a node by id.
    ///
    /// Accepts either a literal `#RGB`/`#RRGGBB` hex OR a `$variable-name`
    /// design-token reference (same rationale as `SetNodeStrokeHex` —
    /// `PenFill::Solid.color` is a free `String`).
    pub(crate) fn cmd_set_node_fill_hex(&mut self, node_id: &NodeId, hex: &str) -> bool {
        if !node_id.is_real() {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        super::borrowed::set_node_fill_hex(node, node_id, hex)
    }

    /// `SetNodeName` — rename a node by id. Rejects whitespace-only
    /// names.
    pub(crate) fn cmd_set_node_name(&mut self, node_id: &NodeId, name: &str) -> bool {
        if !node_id.is_real() {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        super::borrowed::set_node_name(node, node_id, name)
    }

    /// `SetNodeFlag` — flip a boolean flag on a node. `Collapsed` has
    /// no canonical-schema field (it is editor-UI-only state), so
    /// the applier rejects it; `Hidden` writes `visible`, `Locked`
    /// writes `locked`.
    pub(crate) fn cmd_set_node_flag(
        &mut self,
        node_id: &NodeId,
        flag: NodeFlag,
        value: bool,
    ) -> bool {
        if !node_id.is_real() {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        super::borrowed::set_node_flag(node, node_id, flag, value)
    }

    /// `SetNodeFlip` — write the horizontal / vertical mirror flags on
    /// a node. Either axis `None` leaves that flag untouched. Returns
    /// `false` only on a missing node or when nothing was supplied.
    pub(crate) fn cmd_set_node_flip(
        &mut self,
        node_id: &NodeId,
        flip_x: Option<bool>,
        flip_y: Option<bool>,
    ) -> bool {
        if !node_id.is_real() {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        super::borrowed::set_node_flip(node, node_id, flip_x, flip_y)
    }

    /// `SetEllipseArc` — write arc geometry on an Ellipse node.
    /// `start_angle` / `sweep_angle` are degrees; `inner_radius` is a
    /// 0.0..=1.0 fraction. Rejects non-Ellipse kinds, non-finite
    /// values, an out-of-range `inner_radius`, and a call that
    /// supplies nothing.
    pub(crate) fn cmd_set_ellipse_arc(
        &mut self,
        node_id: &NodeId,
        start_angle: Option<f64>,
        sweep_angle: Option<f64>,
        inner_radius: Option<f64>,
    ) -> bool {
        if !node_id.is_real() || !self.is_editable(node_id) {
            return false;
        }
        if start_angle.is_none() && sweep_angle.is_none() && inner_radius.is_none() {
            return false;
        }
        if let Some(a) = start_angle {
            if !a.is_finite() {
                return false;
            }
        }
        if let Some(a) = sweep_angle {
            if !a.is_finite() {
                return false;
            }
        }
        if let Some(r) = inner_radius {
            if !r.is_finite() || !(0.0..=1.0).contains(&r) {
                return false;
            }
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        match node {
            PenNode::Ellipse(e) => {
                if let Some(a) = start_angle {
                    e.start_angle = Some(a);
                }
                if let Some(a) = sweep_angle {
                    // A sweep beyond a full turn just over-draws —
                    // clamp to a single revolution either direction.
                    e.sweep_angle = Some(a.clamp(-360.0, 360.0));
                }
                if let Some(r) = inner_radius {
                    e.inner_radius = Some(r);
                }
                true
            }
            _ => false,
        }
    }

    /// `AddNodeEffect` — append a visual effect with default
    /// parameters. `kind` is `"shadow"` / `"blur"` / `"background_blur"`.
    /// Rejects an unknown kind or a node variant with no effects field
    /// (IconFont / Ref).
    pub(crate) fn cmd_add_node_effect(&mut self, node_id: &NodeId, kind: &str) -> bool {
        if !node_id.is_real() {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        super::borrowed::add_node_effect(node, node_id, kind)
    }

    /// `RemoveNodeEffect` — drop the effect at `index`. Rejects an
    /// out-of-range index; clears the list to `None` once empty so the
    /// serialized `.op` carries no empty `effects` array.
    pub(crate) fn cmd_remove_node_effect(&mut self, node_id: &NodeId, index: u32) -> bool {
        if !node_id.is_real() {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        super::borrowed::remove_node_effect(node, node_id, index)
    }

    /// `SetEffectParam` — write one scalar param of the effect at
    /// `index`. Blur values are clamped to ≥ 0. Rejects a non-finite
    /// value, an out-of-range index, or a field that doesn't match
    /// the effect variant (e.g. `Radius` on a Shadow).
    pub(crate) fn cmd_set_effect_param(
        &mut self,
        node_id: &NodeId,
        index: u32,
        field: EffectField,
        value: f32,
    ) -> bool {
        if !node_id.is_real() {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        super::borrowed::set_effect_param(node, node_id, index, field, value)
    }

    pub(crate) fn cmd_set_effect_visible(
        &mut self,
        node_id: &NodeId,
        index: usize,
        visible: bool,
    ) -> bool {
        if !node_id.is_real() {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        let Some(effects) = node_effects_slot(node).and_then(Option::as_mut) else {
            return false;
        };
        let Some(effect) = effects.get_mut(index) else {
            return false;
        };
        match effect {
            PenEffect::Shadow(body) => body.visible = Some(visible),
            PenEffect::Blur(body) | PenEffect::BackgroundBlur(body) => body.visible = Some(visible),
        }
        true
    }

    pub(crate) fn cmd_set_path_fill_rule(
        &mut self,
        node_id: &NodeId,
        fill_rule: jian_ops_schema::node::path::PathFillRule,
    ) -> bool {
        if !node_id.is_real() {
            return false;
        }
        let Some(PenNode::Path(path)) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        path.fill_rule = Some(fill_rule);
        true
    }

    /// `SetNodeWidgetText` — write a string-typed widget prop
    /// (`placeholder` / `value` / `label`) on whatever widget variant
    /// carries it. Non-widget kinds (and a prop a variant doesn't
    /// have) reject. `value` text widgets keep the value as a plain
    /// string; the NumberInput / Slider numeric `value` is NOT routed
    /// here (use [`cmd_set_node_widget_number`]).
    pub(crate) fn cmd_set_node_widget_text(
        &mut self,
        node_id: &NodeId,
        field: WidgetTextField,
        text: &str,
    ) -> bool {
        if !node_id.is_real() {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        // An empty string clears the optional prop so the serialized
        // `.op` carries no empty placeholder / value / label.
        let next = if text.is_empty() {
            None
        } else {
            Some(text.to_string())
        };
        write_widget_text(node, field, next)
    }

    /// Write the `bind:value` two-way binding on a widget node as
    /// `$state.<key>`. An empty `key` removes the `bind:value` entry
    /// (dropping the whole `bindings` map when it becomes empty) so the
    /// serialized `.op` carries no orphan binding. A `$state.` prefix
    /// the user already typed is stripped so the key isn't doubled.
    /// Returns true only when the node variant carries `bindings`.
    pub(crate) fn cmd_set_node_widget_bind_value(&mut self, node_id: &NodeId, key: &str) -> bool {
        if !node_id.is_real() {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        let Some(slot) = node_bindings_slot(node) else {
            return false;
        };
        let key = key.trim().trim_start_matches("$state.").trim();
        if key.is_empty() {
            if let Some(map) = slot.as_mut() {
                map.remove("bind:value");
                if map.is_empty() {
                    *slot = None;
                }
            }
            return true;
        }
        let expr = jian_ops_schema::expression::Expression(format!("$state.{key}"));
        slot.get_or_insert_with(Default::default)
            .insert("bind:value".to_string(), expr);
        true
    }

    /// `SetNodeWidgetNumber` — write a numeric widget prop (`min` /
    /// `max` / `step` on Slider / NumberInput, or the numeric `value`
    /// on Slider / NumberInput / Progress). Rejects non-finite values
    /// and variants lacking the field.
    pub(crate) fn cmd_set_node_widget_number(
        &mut self,
        node_id: &NodeId,
        field: WidgetNumberField,
        value: f64,
    ) -> bool {
        if !node_id.is_real() || !value.is_finite() {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        write_widget_number(node, field, value)
    }

    /// `ToggleNodeWidgetChecked` — flip the `checked` flag on a Switch
    /// / Checkbox node. Rejects other kinds. An expression-bound
    /// `checked` is overwritten with a literal bool (the panel's
    /// checkbox edits a literal value, parity with TS).
    pub(crate) fn cmd_set_node_widget_checked(&mut self, node_id: &NodeId, checked: bool) -> bool {
        if !node_id.is_real() {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        match node {
            PenNode::Switch(n) => {
                n.checked = Some(BoolOrExpression::Bool(checked));
                true
            }
            PenNode::Checkbox(n) => {
                n.checked = Some(BoolOrExpression::Bool(checked));
                true
            }
            _ => false,
        }
    }

    /// Replace the colour on the Shadow effect at `index`. Blur
    /// kinds carry no colour and silently reject.
    pub(crate) fn cmd_set_effect_color(&mut self, node_id: &NodeId, index: u32, hex: &str) -> bool {
        if !node_id.is_real() {
            return false;
        }
        let Some(node) = find_node_mut(self.active_children_mut(), node_id) else {
            return false;
        };
        super::borrowed::set_effect_color(node, node_id, index, hex)
    }
}
