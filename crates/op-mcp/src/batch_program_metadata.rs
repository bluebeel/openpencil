//! Request-local metadata compiler. Core proves all staged setters succeed;
//! the normal host still owns actual application, history, rollback and ACK.
use super::{parsed_operation, EditorCommand, ToolOutcome};
use crate::{
    batch_design::{find_top_level_char, normalize_node_shape},
    batch_direct_ops::update_command_from_owned_value,
    batch_program_parse::parse_json_arg,
    batch_program_resolve::{
        count_forest, line_preview, resolve_page_index, resolve_path_expr, with_page_id,
    },
};
use jian_ops_schema::node::PenNode;
use op_editor_core::{EditorState, NodeId, PenNodeExt};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

pub(super) fn prepare(
    state: &EditorState,
    lines: &[&str],
    page_id: Option<String>,
    post_process: bool,
) -> Option<ToolOutcome> {
    // Check the complete grammar before parsing patches from a mixed program.
    let bodies: Option<Vec<_>> = lines
        .iter()
        .map(|line| {
            let (binding, op, body) = parsed_operation(line)?;
            (binding.is_none() && op == 'U').then_some(body)
        })
        .collect();
    let bodies = bodies?;
    let page = page_id
        .as_deref()
        .and_then(|id| resolve_page_index(state, id))
        .unwrap_or(state.ui.active_page_index);
    let roots = match state.doc.pages.as_ref().filter(|pages| !pages.is_empty()) {
        Some(pages) => pages[page.min(pages.len() - 1)].children.as_slice(),
        None => state.doc.children.as_slice(),
    };
    if !lines.is_empty() && state.trusted_node_lookup(roots).is_none() {
        return None;
    }
    let bindings = BTreeMap::new();
    let mut parsed = Vec::with_capacity(bodies.len());
    let mut prefixes = Vec::new();
    let mut warnings = Vec::new();
    for (line, body) in lines.iter().zip(bodies) {
        let comma = find_top_level_char(body, ',')?;
        let path = resolve_path_expr(body[..comma].trim(), &bindings);
        let mut value = parse_json_arg(&body[comma + 1..]).ok()?;
        if let Value::Object(patch) = &mut value {
            let mut notes = Vec::new();
            crate::batch_program_dialect::repair_field_dialect(patch, &mut notes);
            for note in notes {
                warnings.push(json!({ "line": line_preview(line), "warning": note }));
            }
        }
        normalize_node_shape(&mut value);
        parsed.push((path, value));
    }
    // Bare IDs need no discovery walk: Core verifies exact existing targets.
    // Slash paths share one warm lookup or one fresh multi-ID traversal.
    for (path, _) in &parsed {
        if path.contains('/') {
            prefixes.push(path.split('/').next()?);
        }
    }
    let found: HashMap<&str, &PenNode> = if prefixes.is_empty() {
        HashMap::new()
    } else {
        let nodes = match state
            .trusted_node_lookup(roots)
            .filter(|lookup| lookup.is_ready())
        {
            Some(lookup) => prefixes
                .iter()
                .filter_map(|id| {
                    lookup
                        .find(id, op_editor_core::NodeReadOrder::First)
                        .map(|entry| entry.node)
                })
                .collect(),
            None => crate::requested_nodes::find(roots, &prefixes),
        };
        nodes
            .into_iter()
            .map(|node| (node.id_str(), node))
            .collect()
    };
    drop(prefixes);
    let mut commands = Vec::with_capacity(parsed.len());
    for (path, value) in parsed {
        let node_id = if path.contains('/') {
            let mut parts = path.split('/');
            let mut node = *found.get(parts.next()?)?;
            for part in parts {
                node = node
                    .children()?
                    .iter()
                    .find(|child| child.id_str() == part)?;
            }
            NodeId::new_opt(node.id_str())?
        } else {
            NodeId::new_opt(path)?
        };
        let command = update_command_from_owned_value(node_id, value).ok()?;
        commands.push(with_page_id(command, page_id.clone()));
    }
    if !commands.is_empty() && !state.preflight_metadata_commands(&commands) {
        return None;
    }
    let node_count = state.trusted_document_node_counts().map_or_else(
        || count_forest(roots),
        |counts| counts.active_page(page).total(),
    );
    let mut envelope = serde_json::Map::new();
    envelope.insert("results".into(), Value::Array(Vec::new()));
    envelope.insert("nodeCount".into(), json!(node_count));
    if post_process {
        envelope.insert("postProcessed".into(), Value::Bool(true));
    }
    if !warnings.is_empty() {
        envelope.insert("warnings".into(), Value::Array(warnings));
    }
    let reply = Value::Object(envelope).to_string();
    Some(match commands.len() {
        0 => ToolOutcome::OkJson(reply),
        1 => ToolOutcome::OkJsonWithCommand(reply, commands.pop()?),
        _ => ToolOutcome::OkJsonWithCommand(reply, EditorCommand::Batch { commands }),
    })
}
