//! `batch_design` operations-DSL parsing: the `I(parent, node)` insert
//! program grammar, its forest assembly, and the shared line/char
//! scanners (`split_operations` / `find_top_level_char`) that the other
//! DSL consumers reuse.
//!
//! Split out of `batch_design.rs` to stay under the 800-line cap.

use std::collections::BTreeMap;

use jian_ops_schema::node::PenNode;
use op_editor_core::{NodeId, PenNodeExt};

use super::batch_design::{ensure_node_ids, normalize_node_shape};
use super::batch_design_dsl_error::InsertDslError;

struct ParsedInsert {
    binding: String,
    parent: ParentRef,
    node: Option<PenNode>,
}

enum ParentRef {
    Root,
    Ref(String),
}

type InsertForest = (NodeId, Vec<PenNode>, usize, Vec<String>);

pub(crate) fn parse_insert_operations(input: &str) -> Result<InsertForest, InsertDslError> {
    let lines = split_operation_slices(input);
    if lines.is_empty() {
        return Err(InsertDslError::NoOperations);
    }
    let mut inserts = Vec::with_capacity(lines.len());
    let mut binding_to_idx = BTreeMap::new();
    let mut tmp_id = 1usize;
    for (line_idx, line) in lines.iter().enumerate() {
        let (binding, parent, data) = parse_insert_operation(line, line_idx)?;
        if binding_to_idx.contains_key(&binding) {
            return Err(InsertDslError::DuplicateBinding(binding));
        }
        let mut value: serde_json::Value =
            serde_json::from_str(data).map_err(|e| InsertDslError::InvalidNodeJson {
                binding: binding.clone(),
                detail: e.to_string(),
            })?;
        normalize_node_shape(&mut value);
        ensure_node_ids(&mut value, &mut tmp_id);
        let mut node: PenNode =
            serde_json::from_value(value).map_err(|e| InsertDslError::InvalidPenNode {
                binding: binding.clone(),
                detail: e.to_string(),
            })?;
        // Stamp the binding as the node's authored id so the post-insert remap
        // (which the `batch_design` tool simulates) can be traced back to its
        // binding for the TS `results:[{binding,nodeId}]` map. The host remaps
        // every id at apply, so this authored id is transient + harmless.
        node.base_mut().id = binding.clone();
        binding_to_idx.insert(binding.clone(), inserts.len());
        inserts.push(ParsedInsert {
            binding,
            parent,
            node: Some(node),
        });
    }
    let bindings: Vec<String> = inserts.iter().map(|i| i.binding.clone()).collect();
    let (parent_id, nodes, count) = assemble_insert_forest(inserts, &binding_to_idx)?;
    Ok((parent_id, nodes, count, bindings))
}

fn assemble_insert_forest(
    mut inserts: Vec<ParsedInsert>,
    binding_to_idx: &BTreeMap<String, usize>,
) -> Result<(NodeId, Vec<PenNode>, usize), InsertDslError> {
    let mut children_by_parent = vec![Vec::<usize>::new(); inserts.len()];
    let mut roots = Vec::<usize>::new();
    let mut real_parent: Option<NodeId> = None;
    for (idx, item) in inserts.iter().enumerate() {
        match &item.parent {
            ParentRef::Root => roots.push(idx),
            ParentRef::Ref(raw) => {
                if let Some(parent_idx) = binding_to_idx.get(raw).copied() {
                    if parent_idx == idx {
                        return Err(InsertDslError::SelfParent(item.binding.clone()));
                    }
                    children_by_parent[parent_idx].push(idx);
                } else {
                    let parent_id = root_or_node_id(raw);
                    if parent_id.is_real() {
                        match &real_parent {
                            Some(existing) if existing != &parent_id => {
                                return Err(InsertDslError::MultipleExistingParents);
                            }
                            None => real_parent = Some(parent_id),
                            _ => {}
                        }
                    }
                    roots.push(idx);
                }
            }
        }
    }
    if roots.is_empty() {
        return Err(InsertDslError::NoRootInsert);
    }
    let mut visit = vec![0u8; inserts.len()];
    let mut nodes = Vec::with_capacity(roots.len());
    let mut stack = Vec::new();
    for root in roots {
        nodes.push(build_tree(
            root,
            &mut inserts,
            &children_by_parent,
            &mut visit,
            &mut stack,
        )?);
    }
    // A disconnected cycle has no root. It must not disappear silently while
    // another component succeeds and nodeCount still reports every operation.
    if visit.iter().any(|state| *state != 2) {
        return Err(InsertDslError::ParentCycle);
    }
    Ok((real_parent.unwrap_or(NodeId::NONE), nodes, inserts.len()))
}

struct BuildFrame {
    index: usize,
    next_child: usize,
    node: PenNode,
}

fn build_tree(
    root: usize,
    inserts: &mut [ParsedInsert],
    children_by_parent: &[Vec<usize>],
    visit: &mut [u8],
    stack: &mut Vec<BuildFrame>,
) -> Result<PenNode, InsertDslError> {
    let take = |index: usize, inserts: &mut [ParsedInsert], visit: &mut [u8]| {
        if visit[index] != 0 {
            return Err(InsertDslError::ParentCycle);
        }
        visit[index] = 1;
        let node = inserts[index]
            .node
            .take()
            .ok_or(InsertDslError::ParentCycle)?;
        Ok(BuildFrame {
            index,
            next_child: 0,
            node,
        })
    };
    stack.push(take(root, inserts, visit)?);
    loop {
        let current = stack.last_mut().ok_or(InsertDslError::ParentCycle)?;
        if let Some(&child) = children_by_parent[current.index].get(current.next_child) {
            current.next_child += 1;
            stack.push(take(child, inserts, visit)?);
            continue;
        }
        let done = stack.pop().ok_or(InsertDslError::ParentCycle)?;
        visit[done.index] = 2;
        if let Some(parent) = stack.last_mut() {
            let children = parent.node.children_mut().ok_or_else(|| {
                InsertDslError::NotAContainer(inserts[parent.index].binding.clone())
            })?;
            children.push(done.node);
        } else {
            return Ok(done.node);
        }
    }
}

fn parse_insert_operation(
    line: &str,
    index: usize,
) -> Result<(String, ParentRef, &str), InsertDslError> {
    let trimmed = line.trim().trim_end_matches(';').trim();
    let (binding, call) = match find_top_level_char(trimmed, '=') {
        Some(eq) => {
            let binding = trimmed[..eq].trim();
            if !is_binding(binding) {
                return Err(InsertDslError::InvalidBinding(binding.to_string()));
            }
            (binding.to_string(), trimmed[eq + 1..].trim())
        }
        None => (format!("_auto_{index}_I"), trimmed),
    };
    if !call.starts_with("I(") || !call.ends_with(')') {
        return Err(InsertDslError::UnsupportedOperation(binding));
    }
    let body = &call[2..call.len() - 1];
    let Some(comma) = find_top_level_char(body, ',') else {
        return Err(InsertDslError::MissingArguments(binding));
    };
    let parent = parse_parent_ref(body[..comma].trim())?;
    let data = body[comma + 1..].trim();
    if data.is_empty() {
        return Err(InsertDslError::EmptyNodeJson(binding));
    }
    Ok((binding, parent, data))
}

/// Returns `true` when a physical line begins a new DSL operation —
/// `name=I(...)`, `I(...)`, `U(...)`, `D(...)`, `M(...)`, `C(...)`, `R(...)`,
/// `G(...)` (with an optional `binding =` prefix). Continuation lines of a
/// pretty-printed JSON body (`"key": value,`) never match, so they accumulate
/// onto the current operation.
fn line_starts_operation(line: &str) -> bool {
    let mut s = line.trim_start();
    if let Some(eq) = s.find('=') {
        let head = s[..eq].trim();
        if !head.is_empty() && head.chars().all(|c| c.is_alphanumeric() || c == '_') {
            s = s[eq + 1..].trim_start();
        }
    }
    let mut chars = s.chars();
    match chars.next() {
        Some('I' | 'C' | 'R' | 'M' | 'G' | 'U' | 'D') => {
            chars.as_str().trim_start().starts_with('(')
        }
        _ => false,
    }
}

/// Split a DSL program into one string per operation. Grouping is by the
/// physical-line operation-start grammar (`line_starts_operation`) rather than
/// a quote/bracket state machine: a weak model that emits an unbalanced quote
/// (e.g. `"fontWeight":"700,"fill"` — `fill` ends up unquoted, an odd number of
/// quotes) used to leak the open-string state across the newline and SWALLOW
/// every following operation into one malformed blob. Anchoring boundaries to
/// the next operation-start line keeps a stray quote contained to its own line
/// (where `parse_json_arg`'s lenient repair can still recover it), and
/// continuation lines of a multi-line JSON body still accumulate correctly.
/// Net bracket delta of a line — `([{` are +1, `)]}` are −1. Strings are NOT
/// tracked on purpose: a weak model's stray quote must not be able to hide a
/// bracket and leak the "open" state across newlines (the bug this guards).
/// A bracket inside a string value is rare and the operation-start guard in
/// `split_operations` recovers it.
fn bracket_delta(line: &str) -> i32 {
    line.chars().fold(0, |d, c| match c {
        '(' | '[' | '{' => d + 1,
        ')' | ']' | '}' => d - 1,
        _ => d,
    })
}

pub(crate) fn split_operations(raw: &str) -> Vec<String> {
    split_operation_slices(raw)
        .into_iter()
        .map(str::to_owned)
        .collect()
}

/// Same recovery boundaries as the owned compatibility API, retaining slices
/// of the original program instead of a growable buffer plus per-line copies.
pub(crate) fn split_operation_slices<'a>(raw: &'a str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut start = None;
    let mut depth = 0i32;
    let mut offset = 0usize;
    let flush = |start: &mut Option<usize>, end: usize, out: &mut Vec<&'a str>| {
        if let Some(begin) = start.take() {
            let line = raw[begin..end].trim();
            if !line.is_empty() && !line.starts_with("//") {
                out.push(line);
            }
        }
    };
    for packet in raw.split_inclusive('\n') {
        let line = packet.strip_suffix('\n').unwrap_or(packet);
        if line_starts_operation(line) && start.is_some() {
            flush(&mut start, offset, &mut out);
            depth = 0;
        }
        let end = offset + line.len();
        let next = offset + packet.len();
        if start.is_none() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with("//") {
                offset = next;
                continue;
            }
            start = Some(offset);
        }
        depth += bracket_delta(line);
        if depth <= 0 {
            flush(&mut start, end, &mut out);
            depth = 0;
        }
        offset = next;
    }
    flush(&mut start, raw.len(), &mut out);
    out
}

pub(crate) fn find_top_level_char(s: &str, target: char) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_string: Option<char> = None;
    let mut escape = false;
    for (idx, ch) in s.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if in_string.is_some() && ch == '\\' {
            escape = true;
            continue;
        }
        if let Some(quote) = in_string {
            if ch == quote {
                in_string = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => in_string = Some(ch),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ if ch == target && depth == 0 => return Some(idx),
            _ => {}
        }
    }
    None
}

fn parse_parent_ref(raw: &str) -> Result<ParentRef, InsertDslError> {
    let raw = raw.trim();
    if matches!(raw, "null" | "undefined" | "\"\"" | "''" | "0" | "\"0\"") {
        return Ok(ParentRef::Root);
    }
    if raw.starts_with('"') {
        return serde_json::from_str::<String>(raw)
            .map(ParentRef::Ref)
            .map_err(|e| InsertDslError::InvalidQuotedParentRef(e.to_string()));
    }
    if raw.starts_with('\'') && raw.ends_with('\'') && raw.len() >= 2 {
        return Ok(ParentRef::Ref(raw[1..raw.len() - 1].to_string()));
    }
    if raw.is_empty() {
        return Ok(ParentRef::Root);
    }
    Ok(ParentRef::Ref(raw.to_string()))
}

fn root_or_node_id(raw: &str) -> NodeId {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "0" {
        NodeId::NONE
    } else {
        NodeId::new(trimmed)
    }
}

fn is_binding(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}
