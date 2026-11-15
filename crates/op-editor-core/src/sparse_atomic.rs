//! Validate context-free scalar commands using complete target headers only.
//! This is not a partial document or a read snapshot. No staged header escapes.

use std::collections::{BTreeMap, HashMap};

use crate::{command_node_attrs::bulk, EditorCommand as C, EditorState, PenNodeExt};
use jian_ops_schema::node::PenNode;
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SparseAtomicValidation {
    /// Context or command semantics require the ordinary isolated simulation.
    Unsupported,
    /// The same command would reject; no source state has changed.
    Rejected { index: usize },
    /// All commands accepted, including the ordinary staged revision behavior.
    Accepted { document_changed: bool },
}

enum Prepared {
    Borrowed(bulk::Prepared),
    Header(Map<String, Value>),
}

/// No document/history clone, allocator, derived scene or partial EditorState.
/// The source authority must remain immutable while the caller plans/commits.
pub fn validate_sparse_atomic_commands(
    source: &EditorState,
    commands: &[C],
) -> SparseAtomicValidation {
    if source.instance_write_virtual_anchor.is_some() || source.editor_ui.workspace.active {
        return SparseAtomicValidation::Unsupported;
    }
    let mut prepared = Vec::with_capacity(commands.len());
    let mut locations = Vec::with_capacity(commands.len());
    let mut wanted: BTreeMap<usize, HashMap<String, usize>> = BTreeMap::new();
    let mut first_invalid_page = commands.len();
    for (index, command) in commands.iter().enumerate() {
        let Some(operation) = prepare(command) else {
            return SparseAtomicValidation::Unsupported;
        };
        let Some(id) = bulk::target(command) else {
            return SparseAtomicValidation::Unsupported;
        };
        let page = match command {
            C::UpdateNode { page_id, .. } | C::PatchNodeData { page_id, .. } => page_id.as_deref(),
            _ => None,
        };
        let page = crate::command_apply::helpers::command_page_index(source, page);
        if let Some(page) = page {
            wanted
                .entry(page)
                .or_default()
                .entry(id.as_str().to_owned())
                .or_insert(index);
        } else {
            first_invalid_page = first_invalid_page.min(index);
        }
        prepared.push(operation);
        locations.push((page, id));
    }
    let mut headers: HashMap<(usize, String), PenNode> = HashMap::new();
    for (page, mut targets) in wanted {
        let roots = match &source.doc.pages {
            Some(pages) if !pages.is_empty() => &pages[page].children,
            _ => &source.doc.children,
        };
        // Ref ancestry is tracked without materializing any master or virtual
        // child. A matched dependency conservatively uses the full simulator.
        let mut stack = vec![(roots.iter(), false)];
        while let Some((siblings, in_ref)) = stack.last_mut() {
            let inside_ref = *in_ref;
            let Some(node) = siblings.next() else {
                stack.pop();
                continue;
            };
            let is_ref = matches!(node, PenNode::Ref(_));
            if targets.remove(node.id_str()).is_some() {
                if inside_ref || is_ref {
                    return SparseAtomicValidation::Unsupported;
                }
                headers.insert(
                    (page, node.id_str().to_owned()),
                    crate::pen_node_ext::clone_without_children(node),
                );
            }
            if targets.is_empty() {
                break;
            }
            if let Some(children) = node.children() {
                stack.push((children.iter(), inside_ref || is_ref));
            }
        }
        // Missing IDs can denote an instance/virtual route. Never infer its
        // semantics from an incomplete target view or fabricate a read result.
        if !targets.is_empty() {
            return SparseAtomicValidation::Unsupported;
        }
    }
    for (index, ((page, id), operation)) in locations.into_iter().zip(prepared).enumerate() {
        if index >= first_invalid_page {
            return SparseAtomicValidation::Rejected {
                index: first_invalid_page,
            };
        }
        if !id.is_real() {
            return SparseAtomicValidation::Rejected { index };
        }
        let Some(node) = headers.get_mut(&(page.expect("validated page"), id.as_str().to_owned()))
        else {
            return SparseAtomicValidation::Unsupported;
        };
        let accepted = match operation {
            Prepared::Borrowed(operation) => bulk::write(node, &commands[index], &operation),
            Prepared::Header(patch) => {
                crate::command_node::borrowed_geometry::patch_header(node, &patch)
            }
        };
        if !accepted {
            return SparseAtomicValidation::Rejected { index };
        }
    }
    // Every accepted member here is a document-marking node writer with no
    // internal history/revision mutation. Match the original saturating counter
    // even after undo or when the counter has reached u64::MAX.
    let document_changed = !commands.is_empty()
        && source
            .revision_counter
            .saturating_add(commands.len() as u64)
            != source.revision;
    SparseAtomicValidation::Accepted { document_changed }
}

fn prepare(command: &C) -> Option<Prepared> {
    if let Some(operation) = bulk::prepare(command) {
        return Some(Prepared::Borrowed(operation));
    }
    let C::PatchNodeData { patch_json, .. } = command else {
        return None;
    };
    let Value::Object(patch) = serde_json::from_str(patch_json).ok()? else {
        return None;
    };
    // Eligibility only: all other commands retain their complete ordinary
    // semantics. Unknown keys are not removed or projected out of a patch.
    patch
        .keys()
        .all(|key| {
            matches!(
                key.as_str(),
                "name"
                    | "x"
                    | "y"
                    | "rotation"
                    | "width"
                    | "height"
                    | "visible"
                    | "locked"
                    | "flipX"
                    | "flipY"
                    | "opacity"
                    | "fill"
                    | "stroke"
                    | "effects"
                    | "cornerRadius"
                    | "content"
                    | "fontFamily"
                    | "fontSize"
                    | "fontWeight"
                    | "fontStyle"
                    | "letterSpacing"
                    | "lineHeight"
                    | "textAlign"
                    | "textAlignVertical"
                    | "fontFeatures"
                    | "fontKerning"
                    | "textDecoration"
                    | "textDecorationSkipInk"
            )
        })
        .then_some(Prepared::Header(patch))
}
