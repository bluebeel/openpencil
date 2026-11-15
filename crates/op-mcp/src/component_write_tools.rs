//! Dedicated Core component mutations; prepared facts borrow the real target.
use super::{EditorCommand, McpTool, ToolErrorCode, ToolOutcome};
use jian_ops_schema::node::PenNode;
use op_editor_core::{EditorState, NodeId, PenNodeExt};
use std::collections::BTreeMap;
const MAX_ID: usize = 16 * 1024;
type Error = (ToolErrorCode, String);
#[derive(Clone, PartialEq, Eq)]
struct Input {
    node: String,
    component: Option<String>,
}
struct Prepared {
    input: Option<Input>,
    result: Result<bool, Error>,
}
fn id(args: &BTreeMap<String, String>, key: &str) -> Result<String, Error> {
    let value = args
        .get(key)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| (ToolErrorCode::MissingArgument, format!("{key} is required")))?;
    if value.len() > MAX_ID {
        return Err((
            ToolErrorCode::InvalidArgument,
            format!("{key} exceeds the 16384-byte ID limit"),
        ));
    }
    Ok(value.clone())
}
fn input(args: &BTreeMap<String, String>, instance: bool) -> Result<Input, Error> {
    Ok(Input {
        node: id(args, "node_id")?,
        component: if instance {
            Some(id(args, "component_id")?)
        } else {
            None
        },
    })
}
// Core component_routes deliberately uses pre-order DFS, not the raw CRUD
// sibling-first walker. Preserve that distinction for duplicate authored IDs.
fn find<'a>(nodes: &'a [PenNode], id: &str) -> Option<&'a PenNode> {
    for node in nodes {
        if node.id_str() == id {
            return Some(node);
        }
        if let Some(found) = node.children().and_then(|children| find(children, id)) {
            return Some(found);
        }
    }
    None
}
fn validate(state: &EditorState, input: &Input) -> Result<bool, Error> {
    let fail = |message: &str| (ToolErrorCode::InvalidArgument, message.to_owned());
    let node = find(state.active_children(), &input.node)
        .ok_or_else(|| fail("node_id does not resolve to a canonical node on the active page"))?;
    if let Some(component) = &input.component {
        let PenNode::Ref(reference) = node else {
            return Err(fail("node_id must be an authored canonical Ref instance"));
        };
        if state
            .components
            .find_by_id(&NodeId::new(component.clone()))
            .is_none()
        {
            return Err(fail("component_id is not a registered component"));
        }
        return Ok(reference.target != *component);
    }
    match node {
        PenNode::Ref(reference) => {
            let lookup =
                op_editor_core::ref_resolve::NodeLookup::from_components_with_document_fallback(
                    &state.components,
                    &state.doc,
                );
            lookup
                .find(&reference.target)
                .ok_or_else(|| fail("instance target cannot be resolved"))?;
        }
        PenNode::Frame(_) | PenNode::Group(_) | PenNode::Rectangle(_)
            if matches!(node,PenNode::Frame(frame) if frame.reusable==Some(true))
                || state
                    .components
                    .find_by_id(&NodeId::new(input.node.clone()))
                    .is_some() => {}
        _ => {
            return Err(fail(
                "node_id is neither a canonical Ref nor a reusable component definition",
            ))
        }
    }
    Ok(true)
}
fn prepare(state: &EditorState, args: &BTreeMap<String, String>, instance: bool) -> Prepared {
    match input(args, instance) {
        Ok(input) => {
            let result = validate(state, &input);
            Prepared {
                input: Some(input),
                result,
            }
        }
        Err(error) => Prepared {
            input: None,
            result: Err(error),
        },
    }
}
fn outcome(
    args: &BTreeMap<String, String>,
    prepared: Option<&Prepared>,
    instance: bool,
) -> ToolOutcome {
    let input = match input(args, instance) {
        Ok(input) => input,
        Err((code, message)) => return ToolOutcome::Err(code, message),
    };
    let mut changed = true;
    if let Some(prepared) = prepared {
        if prepared.input.as_ref() != Some(&input) {
            return ToolOutcome::Err(
                ToolErrorCode::InvalidArgument,
                "prepared component tool arguments changed; build a fresh request snapshot".into(),
            );
        }
        match &prepared.result {
            Ok(value) => changed = *value,
            Err((code, message)) => return ToolOutcome::Err(*code, message.clone()),
        }
    }
    let mut result = BTreeMap::from([("wrote".into(), changed.to_string())]);
    if let Some(component) = input.component {
        result.insert("node_id".into(), input.node.clone());
        result.insert("component_id".into(), component.clone());
        if !changed {
            return ToolOutcome::Ok(result);
        }
        ToolOutcome::OkWithCommand(
            result,
            EditorCommand::SetInstanceComponent {
                node_id: NodeId::new(input.node),
                component_id: NodeId::new(component),
            },
        )
    } else {
        result.insert("original_node_id".into(), input.node.clone());
        result.insert("result_receipt".into(), "unavailable".into());
        ToolOutcome::OkWithCommand(
            result,
            EditorCommand::DetachComponent {
                node_id: NodeId::new(input.node),
            },
        )
    }
}
pub struct DetachComponent {
    prepared: Option<Prepared>,
}
impl McpTool for DetachComponent {
    fn name(&self) -> &str {
        "detach_component"
    }
    fn call(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        outcome(args, self.prepared.as_ref(), false)
    }
}
pub fn detach_component_snapshot() -> DetachComponent {
    DetachComponent { prepared: None }
}
pub fn detach_component_snapshot_for_call(
    state: &EditorState,
    args: &BTreeMap<String, String>,
) -> DetachComponent {
    DetachComponent {
        prepared: Some(prepare(state, args, false)),
    }
}
pub struct SetInstanceComponent {
    prepared: Option<Prepared>,
}
impl McpTool for SetInstanceComponent {
    fn name(&self) -> &str {
        "set_instance_component"
    }
    fn call(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        outcome(args, self.prepared.as_ref(), true)
    }
}
pub fn set_instance_component_snapshot() -> SetInstanceComponent {
    SetInstanceComponent { prepared: None }
}
pub fn set_instance_component_snapshot_for_call(
    state: &EditorState,
    args: &BTreeMap<String, String>,
) -> SetInstanceComponent {
    SetInstanceComponent {
        prepared: Some(prepare(state, args, true)),
    }
}
