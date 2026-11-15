//! Virtual component-instance child lookup for inspector reads/writes.
//!
//! Canvas expansion gives every authored component descendant a
//! render-only `refId__childId` anchor. This module validates that id
//! against the real Ref + component tree and resolves the effective
//! child without teaching generic document walkers about virtual nodes.

use crate::node_id::NodeId;
use crate::pen_node_ext::PenNodeExt;
use crate::walkers::find_node;
use jian_ops_schema::node::PenNode;
use jian_ops_schema::PenDocument;

/// Split a canvas instance-child anchor into its authored Ref id and
/// original component-child id. A delimiter match is not enough: the
/// candidate must correspond to a resolvable Ref and a real descendant
/// of that Ref's rendered child source.
///
/// Nested expanded refs (`outer__inner__leaf`) are intentionally not
/// resolved yet. They have no single authored Ref node to receive the
/// override; returning `None` keeps that unsupported case read-only.
pub fn split_instance_child_anchor(anchor: &NodeId, doc: &PenDocument) -> Option<(NodeId, NodeId)> {
    split_with_lookup(anchor, &crate::ref_resolve::NodeLookup::from_document(doc))
}
fn split_with_lookup(
    anchor: &NodeId,
    lookup: &crate::ref_resolve::NodeLookup<'_>,
) -> Option<(NodeId, NodeId)> {
    fn contains_descendant(children: &[PenNode], child_id: &NodeId) -> bool {
        for child in children {
            if child.id_str() == child_id.as_str() {
                return true;
            }
            if child
                .children()
                .is_some_and(|grandchildren| contains_descendant(grandchildren, child_id))
            {
                return true;
            }
        }
        false
    }

    let raw = anchor.as_str();
    if !anchor.is_real() || !raw.contains("__") || lookup.find_authored(raw).is_some() {
        return None;
    }
    let mut found = None;
    let mut ambiguous = false;
    // Check every adjacent underscore pair. `str::match_indices` advances
    // past a match and therefore misses the real boundary in an id such as
    // `inst___icon` (`ref_id = "inst_"`, child id = "icon").
    for separator in raw
        .as_bytes()
        .windows(2)
        .enumerate()
        .filter_map(|(index, pair)| (pair == b"__").then_some(index))
    {
        let (ref_raw, child_with_separator) = raw.split_at(separator);
        let child_raw = &child_with_separator[2..];
        let (Some(ref_id), Some(child_id)) = (NodeId::new_opt(ref_raw), NodeId::new_opt(child_raw))
        else {
            continue;
        };
        let Some(ref_node @ PenNode::Ref(reference)) = lookup.find_authored(ref_id.as_str()) else {
            continue;
        };
        let Some(component) = lookup.find(&reference.target) else {
            continue;
        };
        let component_children = component.children().filter(|children| !children.is_empty());
        let Some(children) = component_children.or_else(|| ref_node.children()) else {
            continue;
        };
        if !contains_descendant(children, &child_id)
            || crate::ref_resolve::instance_child_virtual_id(ref_id.as_str(), child_id.as_str())
                != raw
        {
            continue;
        }
        let candidate = (ref_id, child_id);
        if found
            .as_ref()
            .is_some_and(|existing| existing != &candidate)
        {
            ambiguous = true;
        } else {
            found = Some(candidate);
        }
    }
    (!ambiguous).then_some(found).flatten()
}

/// Resolve an authored Ref root or one of its canvas-only virtual
/// child anchors into the effective node shown by the inspector.
/// Child results keep the virtual anchor id while all other fields
/// come from the component child plus `descendants[child_id]`.
pub fn resolve_instance_display_node_for_anchor(
    doc: &PenDocument,
    anchor: &NodeId,
) -> Option<PenNode> {
    resolve_with_lookup(
        &crate::ref_resolve::NodeLookup::from_document(doc),
        anchor,
        false,
    )
}
fn resolve_with_lookup(
    lookup: &crate::ref_resolve::NodeLookup<'_>,
    anchor: &NodeId,
    header: bool,
) -> Option<PenNode> {
    if let Some(node @ PenNode::Ref(reference)) = lookup.find_authored(anchor.as_str()) {
        return crate::ref_resolve::merge_instance(
            node,
            lookup.find(&reference.target)?,
            true,
            header,
        );
    }
    let (ref_id, child_id) = split_with_lookup(anchor, lookup)?;
    let node @ PenNode::Ref(reference) = lookup.find_authored(ref_id.as_str())? else {
        return None;
    };
    let display =
        crate::ref_resolve::merge_instance(node, lookup.find(&reference.target)?, true, false)?;
    let child = find_node(display.children()?, &child_id)?;
    let mut child = if header {
        crate::pen_node_ext::clone_without_children(child)
    } else {
        child.clone()
    };
    child.base_mut().id = anchor.as_str().to_owned();
    Some(child)
}
impl crate::EditorState {
    /// Inspector metadata uses the same registry precedence as canvas Ref
    /// expansion, including owned external prototypes absent from the Doc.
    pub fn instance_display_header(&self, anchor: &NodeId) -> Option<PenNode> {
        if find_node(self.active_children(), anchor)
            .is_some_and(|node| !matches!(node, PenNode::Ref(_)))
        {
            return None;
        }

        let lookup = crate::ref_resolve::NodeLookup::from_components_with_document_fallback(
            &self.components,
            &self.doc,
        );
        if let Some(node @ PenNode::Ref(reference)) = find_node(self.active_children(), anchor) {
            return crate::ref_resolve::merge_instance(
                node,
                lookup.find(&reference.target)?,
                true,
                true,
            );
        }
        resolve_with_lookup(&lookup, anchor, true)
    }
    pub(crate) fn split_instance_anchor(&self, anchor: &NodeId) -> Option<(NodeId, NodeId)> {
        if !anchor.as_str().contains("__") {
            return None;
        }
        let lookup = crate::ref_resolve::NodeLookup::from_components_with_document_fallback(
            &self.components,
            &self.doc,
        );
        split_with_lookup(anchor, &lookup)
    }
}
pub(crate) fn prepare_instance_write(
    state: &crate::EditorState,
    anchor: &NodeId,
) -> Option<(NodeId, String, PenNode, bool)> {
    let lookup = crate::ref_resolve::NodeLookup::from_components_with_document_fallback(
        &state.components,
        &state.doc,
    );
    if let Some(node @ PenNode::Ref(reference)) = find_node(state.active_children(), anchor) {
        let base = node.base();
        if !base.visible.unwrap_or(true) || base.locked.unwrap_or(false) {
            return None;
        }
        return Some((
            anchor.clone(),
            reference.target.clone(),
            crate::ref_resolve::merge_instance(node, lookup.find(&reference.target)?, true, true)?,
            true,
        ));
    }
    let (ref_id, child_id) = split_with_lookup(anchor, &lookup)?;
    let node = find_node(state.active_children(), &ref_id)?;
    // A virtual anchor must not bind a different same-id Ref on another page.
    if !std::ptr::eq(node, lookup.find_authored(ref_id.as_str())?) {
        return None;
    }
    let base = node.base();
    if !base.visible.unwrap_or(true) || base.locked.unwrap_or(false) {
        return None;
    }
    Some((
        ref_id,
        child_id.as_str().to_owned(),
        resolve_with_lookup(&lookup, anchor, true)?,
        false,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::walkers::{find_node, find_node_mut};
    use crate::EditorState;

    const DOC: &str = r##"{
      "version":"1.0.0",
      "children":[
        {"type":"frame","id":"button","name":"Button","reusable":true,
         "width":120,"height":40,"children":[
           {"type":"icon_font","id":"icon","name":"home","iconFontName":"home",
            "width":24,"height":24,"fill":[{"type":"solid","color":"#111111"}]}
         ]},
        {"type":"ref","id":"inst","ref":"button","x":200,"y":80}
      ]
    }"##;

    fn state() -> EditorState {
        let doc = jian_ops_schema::load_str(DOC).expect("fixture").value;
        EditorState::from_document(doc)
    }

    fn rich_color_state() -> EditorState {
        let doc = jian_ops_schema::load_str(
            r##"{
              "version":"1.0.0",
              "children":[
                {"type":"frame","id":"card","name":"Card","reusable":true,
                 "width":120,"height":80,"children":[
                   {"type":"rectangle","id":"paint","name":"Paint",
                    "width":80,"height":40,
                    "fill":[
                      {"type":"linear_gradient","angle":0,"stops":[
                        {"offset":0,"color":"#ffffff"},
                        {"offset":1,"color":"#00000080"}
                      ]},
                      {"type":"solid","color":"#123456"}
                    ],
                    "effects":[{"type":"shadow","offsetX":0,"offsetY":4,
                      "blur":8,"spread":0,"color":"#00000040"}]}
                 ]},
                {"type":"ref","id":"inst","ref":"card","x":200,"y":80}
              ]
            }"##,
        )
        .expect("rich color fixture")
        .value;
        EditorState::from_document(doc)
    }

    fn ref_node(state: &EditorState) -> &jian_ops_schema::node::RefNode {
        match find_node(state.active_children(), &NodeId::new("inst")) {
            Some(PenNode::Ref(reference)) => reference,
            other => panic!("inst must remain a Ref, got {other:?}"),
        }
    }

    #[test]
    fn picker_opens_edits_and_records_history_for_virtual_child() {
        let mut state = state();
        state.set_single_selection(NodeId::new("inst__icon"));
        assert!(state.open_color_picker(crate::ui_draft::ColorTarget::Fill, 0.0));
        assert!(state.color_picker_set_hsv(0.0, 1.0, 1.0));
        assert!(state.close_color_picker());
        assert_eq!(state.history.past.len(), 1);
        assert_eq!(
            ref_node(&state)
                .descendants
                .as_ref()
                .and_then(|d| d.get("icon"))
                .and_then(|v| v.pointer("/fill/0/color"))
                .and_then(serde_json::Value::as_str),
            Some("#ff0000")
        );
    }

    #[test]
    fn picker_routes_every_node_color_target_to_virtual_child_override() {
        let mut indexed = rich_color_state();
        indexed.set_single_selection(NodeId::new("inst__paint"));
        assert!(indexed.open_color_picker_for_fill(crate::ui_draft::ColorTarget::Fill, 1, 0.0));
        assert!(indexed.color_picker_set_hsv(0.0, 1.0, 1.0));
        assert_eq!(
            ref_node(&indexed)
                .descendants
                .as_ref()
                .and_then(|d| d.get("paint"))
                .and_then(|v| v.pointer("/fill/1/color"))
                .and_then(serde_json::Value::as_str),
            Some("#ff0000")
        );

        let mut gradient = rich_color_state();
        gradient.set_single_selection(NodeId::new("inst__paint"));
        assert!(gradient.open_color_picker(crate::ui_draft::ColorTarget::GradientStop(1), 0.0));
        assert!(gradient.color_picker_set_hsv(120.0, 1.0, 1.0));
        assert_eq!(
            ref_node(&gradient)
                .descendants
                .as_ref()
                .and_then(|d| d.get("paint"))
                .and_then(|v| v.pointer("/fill/0/stops/1/color"))
                .and_then(serde_json::Value::as_str),
            Some("#00ff0080")
        );

        let mut effect = rich_color_state();
        effect.set_single_selection(NodeId::new("inst__paint"));
        assert!(effect.open_color_picker(crate::ui_draft::ColorTarget::EffectColor(0), 0.0));
        effect.color_picker_focus_hex();
        effect.color_picker_hex_backspace(0);
        effect.color_picker_hex_char('f', 1);
        assert_eq!(
            ref_node(&effect)
                .descendants
                .as_ref()
                .and_then(|d| d.get("paint"))
                .and_then(|v| v.pointer("/effects/0/color"))
                .and_then(serde_json::Value::as_str),
            Some("#00000f40")
        );
    }

    #[test]
    fn split_accepts_ref_id_ending_in_underscore() {
        let mut state = state();
        let PenNode::Ref(reference) = find_node(state.active_children(), &NodeId::new("inst"))
            .expect("ref")
            .clone()
        else {
            panic!("expected ref");
        };
        let mut underscored = reference;
        underscored.base.id = "inst_".into();
        state.doc.children.push(PenNode::Ref(underscored));

        assert_eq!(
            split_instance_child_anchor(&NodeId::new("inst___icon"), &state.doc),
            Some((NodeId::new("inst_"), NodeId::new("icon")))
        );
    }

    #[test]
    fn direct_fill_opacity_write_routes_to_virtual_child_override() {
        let mut state = state();
        state.set_single_selection(NodeId::new("inst__icon"));
        assert!(state.set_selected_fill_opacity(0.25));
        assert_eq!(
            ref_node(&state)
                .descendants
                .as_ref()
                .and_then(|d| d.get("icon"))
                .and_then(|v| v.pointer("/fill/0/opacity"))
                .and_then(serde_json::Value::as_f64),
            Some(0.25)
        );
    }

    #[test]
    fn history_repair_handles_a_full_past_deque() {
        let mut state = state();
        for _ in 0..crate::HISTORY_CAP {
            state.commit_history();
        }
        let child = NodeId::new("inst__icon");
        state.set_single_selection(child.clone());
        let scope = state
            .begin_instance_write_for_anchor()
            .expect("virtual child scope");
        state.commit_history();
        assert!(state.set_selected_color(true, "#ff0000"));
        state.finish_instance_write(scope);

        assert_eq!(state.history.past.len(), crate::HISTORY_CAP);
        let newest = state
            .history
            .past
            .back()
            .expect("newest history snapshot")
            .full_snapshot()
            .unwrap();
        assert!(matches!(
            newest
                .doc
                .snapshot_find_node(0, &NodeId::new("inst"))
                .map(crate::SharedNode::metadata),
            Some(PenNode::Ref(_))
        ));
        assert!(newest.doc.snapshot_find_node(0, &child).is_none());
    }

    #[test]
    fn authored_node_wins_over_a_colliding_virtual_id() {
        let mut state = state();
        let ordinary: PenNode = serde_json::from_value(serde_json::json!({
            "type":"rectangle", "id":"inst__icon", "name":"Authored",
            "width":40, "height":40,
            "fill":[{"type":"solid","color":"#222222"}]
        }))
        .expect("ordinary authored node");
        state.doc.children.push(ordinary);
        state.set_single_selection(NodeId::new("inst__icon"));

        assert!(
            resolve_instance_display_node_for_anchor(&state.doc, &NodeId::new("inst__icon"))
                .is_none()
        );
        assert!(state.set_selected_color(true, "#00ff00"));
        let ordinary = find_node(state.active_children(), &NodeId::new("inst__icon"))
            .expect("authored node remains selected");
        assert_eq!(
            crate::fills::first_solid_fill_hex(ordinary),
            Some("#00ff00")
        );
        assert!(ref_node(&state).descendants.is_none());

        let master_icon = find_node_mut(state.active_children_mut(), &NodeId::new("icon"))
            .expect("master icon remains untouched");
        assert_eq!(
            crate::fills::first_solid_fill_hex(master_icon),
            Some("#111111")
        );
    }
}
