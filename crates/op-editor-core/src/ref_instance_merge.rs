//! Owned JSON merge: each instance subtree is serialized/transformed once.
use super::*;
use crate::pen_node_ext::PenNodeExt;

/// Preserve the previous spread order, including structural descendant
/// overrides, while moving child forests instead of cloning every ancestor.
pub(super) fn descendants(
    child: Value,
    overrides: Option<&Map<String, Value>>,
    ref_id: Option<&str>,
) -> Value {
    let Value::Object(mut mapped) = child else {
        return child;
    };
    let original_id = mapped
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if let Some(override_map) = overrides
        .and_then(|map| map.get(&original_id))
        .and_then(Value::as_object)
    {
        for (key, value) in override_map {
            mapped.insert(key.clone(), value.clone());
        }
    }
    if let Some(ref_id) = ref_id {
        mapped.insert(
            "id".into(),
            Value::String(instance_child_virtual_id(ref_id, &original_id)),
        );
    }
    if let Some(Value::Array(children)) = mapped.get_mut("children") {
        for child in children {
            *child = descendants(std::mem::take(child), overrides, ref_id);
        }
    }
    Value::Object(mapped)
}

/// Header-only mode never materializes a component's descendant forest. It
/// retains the same empty/absent child slot exposed by the existing write scope.
pub(crate) fn merge(
    ref_node: &PenNode,
    component: &PenNode,
    display_id: bool,
    header_only: bool,
) -> Option<PenNode> {
    merge_inner(ref_node, component, display_id, header_only, None)
}
pub(super) fn expand(ref_node: &PenNode, component: &PenNode) -> Option<PenNode> {
    merge_inner(
        ref_node,
        component,
        true,
        false,
        Some(ref_node.base().id.as_str()),
    )
}
fn merge_inner(
    ref_node: &PenNode,
    component: &PenNode,
    display_id: bool,
    header_only: bool,
    virtual_id: Option<&str>,
) -> Option<PenNode> {
    let component_value = if header_only {
        serde_json::to_value(crate::history_snapshot::BorrowedNodeMetadata::new(
            component,
        ))
        .ok()?
    } else {
        serde_json::to_value(component).ok()?
    };
    let reference_value = if header_only {
        serde_json::to_value(crate::history_snapshot::BorrowedNodeMetadata::new(ref_node)).ok()?
    } else {
        serde_json::to_value(ref_node).ok()?
    };
    let (Value::Object(mut merged), Value::Object(mut reference)) =
        (component_value, reference_value)
    else {
        return None;
    };
    let original_name = merged.get("name").cloned();
    // Keep the original forests separate from top-level overrides. Root
    // overrides cannot replace children; per-descendant overrides still can.
    let component_children = merged.remove("children");
    let inline_children = reference.remove("children");
    apply_top_level_overrides(&mut merged, &reference);
    for (key, value) in &reference {
        if matches!(key.as_str(), "type" | "ref" | "descendants" | "children")
            || (!display_id && key == "id")
        {
            continue;
        }
        if !value.is_null() {
            merged.insert(key.clone(), value.clone());
        }
    }
    if merged.get("name").is_none_or(Value::is_null) {
        if let Some(name) = original_name {
            merged.insert("name".into(), name);
        }
    }
    merged.remove("reusable");
    if header_only {
        if component.children().is_some() || ref_node.children().is_some() {
            merged.insert("children".into(), Value::Array(Vec::new()));
        }
    } else {
        let children = match component_children {
            Some(Value::Array(children)) if !children.is_empty() => Some(children),
            // Empty styled masters use each instance's inline slot content.
            empty => match inline_children {
                Some(Value::Array(children)) => Some(children),
                _ => empty.and_then(|value| match value {
                    Value::Array(children) => Some(children),
                    _ => None,
                }),
            },
        };
        if let Some(children) = children {
            let overrides = reference.get("descendants").and_then(Value::as_object);
            merged.insert(
                "children".into(),
                Value::Array(
                    children
                        .into_iter()
                        .map(|child| descendants(child, overrides, virtual_id))
                        .collect(),
                ),
            );
        }
    }
    serde_json::from_value(Value::Object(merged)).ok()
}
