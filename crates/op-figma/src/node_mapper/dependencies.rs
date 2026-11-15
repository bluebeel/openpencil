//! Conservative master closure: inspect every typed GUID, including unknown containers.
use crate::figma_types::FigGuid;
use crate::kiwi::FigValue;
use crate::tree::TreeNode;
use std::collections::{HashMap, HashSet};
fn value(value: &FigValue, symbols: &HashMap<String, &TreeNode>, out: &mut Vec<String>) {
    let value = value.resolved();
    if let Some(guid) = FigGuid::from_value(value) {
        let key = guid.to_key();
        if symbols.contains_key(&key) {
            out.push(key)
        }
    }
    match value {
        FigValue::Array(values) => {
            for child in values {
                self::value(child, symbols, out)
            }
        }
        FigValue::Object(fields) => {
            for (_, child) in fields {
                self::value(child, symbols, out)
            }
        }
        FigValue::Deferred(_) => unreachable!("resolved cache"),
        FigValue::Null
        | FigValue::Bool(_)
        | FigValue::Int(_)
        | FigValue::Uint(_)
        | FigValue::Int64(_)
        | FigValue::Uint64(_)
        | FigValue::Float(_)
        | FigValue::Str(_)
        | FigValue::Symbol(_)
        | FigValue::Bytes(_)
        | FigValue::Guid(_, _)
        | FigValue::Affine(_)
        | FigValue::Vec2(_)
        | FigValue::Color(_)
        | FigValue::TextEvidence(_)
        | FigValue::TextLayout(_) => {}
    }
}
fn node(node: &TreeNode, symbols: &HashMap<String, &TreeNode>, out: &mut Vec<String>) {
    value(&node.figma, symbols, out);
    for child in &node.children {
        self::node(child, symbols, out)
    }
}
pub(super) fn closure(page: &TreeNode, symbols: &HashMap<String, &TreeNode>) -> HashSet<String> {
    let mut pending = Vec::new();
    node(page, symbols, &mut pending);
    let mut wanted = HashSet::new();
    while let Some(key) = pending.pop() {
        if wanted.insert(key.clone()) {
            if let Some(master) = symbols.get(&key) {
                node(master, symbols, &mut pending)
            }
        }
    }
    wanted
}
