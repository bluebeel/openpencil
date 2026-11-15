//! Prompt-size and plan-overlap guards for the AI codegen pipeline.

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::Value;

use crate::ai::types::{ExecutableChunk, PlannedChunk};
use op_editor_core::codegen::{ChunkStatus, Framework};

pub(super) const MAX_USER_PROMPT_BYTES: usize = 120_000;

pub(super) fn is_pascal_case(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_uppercase())
        && chars.all(|char| char.is_ascii_alphanumeric())
}

pub(super) fn assembly_status_label(status: ChunkStatus) -> &'static str {
    match status {
        ChunkStatus::Done => "successful",
        ChunkStatus::Degraded => "degraded",
        _ => "failed",
    }
}

/// Reject only obvious prose/acknowledgements from final model phases. The
/// markers are deliberately permissive: this is a last quality gate, not a
/// parser or compiler, so unusual but structurally recognizable code passes.
pub(super) fn model_output_failure(framework: Framework, code: &str) -> Option<String> {
    if code.trim().is_empty() {
        return Some("model returned empty code".to_string());
    }
    (!plausible_code(framework, code)).then(|| {
        format!(
            "model returned text without recognizable {} code structure",
            framework.as_wire()
        )
    })
}

fn plausible_code(framework: Framework, code: &str) -> bool {
    let has_any = |needles: &[&str]| needles.iter().any(|needle| code.contains(needle));
    let function_block = code.contains("function ") && code.contains('{') && code.contains('}');
    let markup = has_any(&[
        "</",
        "/>",
        "<html",
        "<body",
        "<template",
        "<div",
        "<main",
        "<section",
        "<form",
        "<input",
        "<button",
        "<svg",
    ]);
    match framework {
        Framework::React | Framework::ReactTailwind => {
            markup
                || has_any(&["export default", "from 'react'", "from \"react\"", "React."])
                || function_block
        }
        Framework::ReactNative => {
            markup
                || has_any(&[
                    "react-native",
                    "StyleSheet.create",
                    "export default",
                    "React.",
                ])
                || function_block
        }
        Framework::Vue => {
            markup
                || has_any(&[
                    "<script",
                    "defineComponent(",
                    "createApp(",
                    "export default {",
                ])
        }
        Framework::Svelte => markup || has_any(&["<script", "<style", "<svelte:"]),
        Framework::Html => {
            markup || has_any(&["<!DOCTYPE", "<!doctype", "<head", "<p", "<span", "<img"])
        }
        Framework::Flutter => has_any(&[
            "package:flutter/",
            "Widget build(",
            "runApp(",
            "StatelessWidget",
            "StatefulWidget",
            "MaterialApp(",
            "Scaffold(",
        ]),
        Framework::SwiftUi => has_any(&[
            "import SwiftUI",
            ": View",
            "var body: some View",
            "#Preview",
            "VStack(",
            "HStack(",
            "ZStack(",
        ]),
        Framework::Compose => has_any(&[
            "@Composable",
            "setContent {",
            "Modifier.",
            "androidx.compose",
            "MaterialTheme",
        ]),
    }
}

pub(super) fn index_node_ids(value: &Value, out: &mut HashSet<String>) {
    match value {
        Value::Array(items) => {
            for item in items {
                index_node_ids(item, out);
            }
        }
        Value::Object(map) => {
            if let Some(id) = map.get("id").and_then(Value::as_str) {
                out.insert(id.to_string());
            }
            if let Some(children) = map.get("children") {
                index_node_ids(children, out);
            }
        }
        _ => {}
    }
}

pub(super) fn chunk_nodes_json(
    node_forest: &Value,
    fallback_json: &str,
    node_ids: &[String],
) -> String {
    let found = find_requested_values(node_forest, node_ids.iter().map(String::as_str));
    let subset = node_ids
        .iter()
        .filter_map(|id| found.get(id.as_str()).copied())
        .collect::<Vec<_>>();
    if subset.is_empty() {
        return fallback_json.to_string();
    }
    serde_json::to_string(&subset).unwrap_or_else(|_| fallback_json.to_string())
}

/// Compact metadata for wrappers above the selected chunk roots. Partitioned
/// fallback plans intentionally select an antichain of descendants, so this
/// preserves their parents' geometry and visual properties without copying
/// sibling subtrees into every chunk request.
pub(super) fn ancestor_context_json(node_forest: &Value, node_ids: &[String]) -> Option<String> {
    let targets = node_ids.iter().map(String::as_str).collect::<HashSet<_>>();
    let mut contexts = HashMap::<&str, Vec<&Value>>::new();
    collect_ancestor_contexts(node_forest, &targets, &mut Vec::new(), &mut contexts);
    let mut wrappers = HashMap::<usize, Value>::new();
    let mut grouped = Vec::<(Vec<Value>, Vec<String>)>::new();
    let mut group_indices = HashMap::<String, usize>::new();
    for node_id in node_ids {
        let Some(path) = contexts
            .get(node_id.as_str())
            .filter(|path| !path.is_empty())
        else {
            continue;
        };
        let ancestors = path
            .iter()
            .map(|value| {
                wrappers
                    .entry(*value as *const Value as usize)
                    .or_insert_with(|| {
                        let map = value.as_object().expect("captured ancestor object");
                        // Preserve field order/unknown metadata, excluding child trees
                        // BEFORE copying. The old clone-then-remove copied every child.
                        let mut wrapper = map
                            .iter()
                            .filter(|(key, _)| key.as_str() != "children")
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect::<serde_json::Map<_, _>>();
                        let count = map
                            .get("children")
                            .and_then(Value::as_array)
                            .map_or(0, Vec::len);
                        wrapper.insert("childCount".into(), Value::from(count as u64));
                        let mut wrapper = Value::Object(wrapper);
                        super::prompts::strip_ancestor_noise(&mut wrapper);
                        wrapper
                    })
                    .clone()
            })
            .collect::<Vec<_>>();
        // Group by the same serialized metadata, not address equality: distinct
        // duplicate wrappers with equal content retain the original grouping.
        let key = serde_json::to_string(&ancestors).ok()?;
        let index = *group_indices.entry(key).or_insert_with(|| {
            grouped.push((ancestors, Vec::new()));
            grouped.len() - 1
        });
        if !grouped[index].1.contains(node_id) {
            grouped[index].1.push(node_id.clone());
        }
    }
    if grouped.is_empty() {
        return None;
    }
    let payload = grouped
        .into_iter()
        .map(|(ancestors, node_ids)| serde_json::json!({"nodeIds":node_ids,"ancestors":ancestors}))
        .collect::<Vec<_>>();
    serde_json::to_string(&payload).ok()
}
fn collect_ancestor_contexts<'a>(
    value: &'a Value,
    targets: &HashSet<&str>,
    path: &mut Vec<&'a Value>,
    out: &mut HashMap<&'a str, Vec<&'a Value>>,
) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_ancestor_contexts(item, targets, path, out);
            }
        }
        Value::Object(map) => {
            if let Some(id) = map.get("id").and_then(Value::as_str) {
                if targets.contains(id) {
                    out.entry(id).or_insert_with(|| path.clone());
                }
            }
            let Some(children) = map.get("children").and_then(Value::as_array) else {
                return;
            };
            path.push(value);
            for child in children {
                collect_ancestor_contexts(child, targets, path, out);
            }
            path.pop();
        }
        _ => {}
    }
}

/// Validate planner-owned graph fields before execution-order calculation or
/// request dispatch. Duplicate ids make request completion ambiguous, while
/// missing/cyclic dependencies can leave chunks permanently undispatchable.
pub(super) fn plan_structure_issue(chunks: &[PlannedChunk]) -> Option<String> {
    let mut ids = HashSet::with_capacity(chunks.len());
    for chunk in chunks {
        if chunk.id.trim().is_empty() {
            return Some("a chunk has an empty id".to_string());
        }
        if !ids.insert(chunk.id.as_str()) {
            return Some(format!("duplicate chunk id {}", chunk.id));
        }
    }

    let mut indegrees = HashMap::with_capacity(chunks.len());
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    for chunk in chunks {
        let mut unique_dependencies = HashSet::new();
        for dependency in &chunk.dependencies {
            if !ids.contains(dependency.as_str()) {
                return Some(format!(
                    "chunk {} depends on unknown chunk {dependency}",
                    chunk.id
                ));
            }
            if unique_dependencies.insert(dependency.as_str()) {
                dependents
                    .entry(dependency.as_str())
                    .or_default()
                    .push(chunk.id.as_str());
            }
        }
        indegrees.insert(chunk.id.as_str(), unique_dependencies.len());
    }

    let mut ready = chunks
        .iter()
        .filter(|chunk| indegrees.get(chunk.id.as_str()) == Some(&0))
        .map(|chunk| chunk.id.as_str())
        .collect::<VecDeque<_>>();
    let mut visited = 0;
    while let Some(id) = ready.pop_front() {
        visited += 1;
        for dependent in dependents.get(id).into_iter().flatten() {
            let Some(indegree) = indegrees.get_mut(dependent) else {
                continue;
            };
            *indegree -= 1;
            if *indegree == 0 {
                ready.push_back(dependent);
            }
        }
    }
    if visited != chunks.len() {
        let cycle = chunks
            .iter()
            .filter(|chunk| {
                indegrees
                    .get(chunk.id.as_str())
                    .is_some_and(|degree| *degree > 0)
            })
            .map(|chunk| chunk.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Some(format!("chunk dependency cycle involving {cycle}"));
    }
    None
}

pub(super) fn plan_dispatch_issue(
    chunks: &[ExecutableChunk],
    node_forest: &Value,
) -> Option<String> {
    let claimed = chunks
        .iter()
        .flat_map(|chunk| {
            chunk
                .plan
                .node_ids
                .iter()
                .map(move |node_id| (chunk.plan.id.as_str(), node_id.as_str()))
        })
        .collect::<Vec<_>>();
    if claimed.len() < 2 {
        return None;
    }
    let targets = claimed.iter().map(|(_, id)| *id).collect::<HashSet<_>>();
    let found = find_requested_values(node_forest, targets.iter().copied());
    let mut descendants = HashMap::<usize, HashSet<&str>>::new();
    for (index, (left_chunk, left_id)) in claimed.iter().enumerate() {
        for (right_chunk, right_id) in claimed.iter().skip(index + 1) {
            if left_id == right_id {
                return Some(format!(
                    "chunks {left_chunk} and {right_chunk} both claim node {left_id}"
                ));
            }
            let mut contains = |id: &str, target: &str| {
                found.get(id).is_some_and(|node| {
                    descendants
                        .entry(*node as *const Value as usize)
                        .or_insert_with(|| {
                            let mut wanted = HashSet::new();
                            collect_descendant_targets(node, &targets, &mut wanted);
                            wanted
                        })
                        .contains(target)
                })
            };
            let overlaps = contains(left_id, right_id) || contains(right_id, left_id);
            if overlaps {
                return Some(format!(
                    "chunks {left_chunk} and {right_chunk} claim overlapping ancestor/descendant nodes {left_id} and {right_id}"
                ));
            }
        }
    }
    None
}

/// Same first-DFS semantics as the old per-ID search, including unusual
/// JSON children shapes; stop once every requested ID has its first match.
fn find_requested_values<'a, 'b>(
    value: &'a Value,
    ids: impl IntoIterator<Item = &'b str>,
) -> HashMap<&'a str, &'a Value> {
    fn visit<'a>(
        value: &'a Value,
        wanted: &mut HashSet<&str>,
        out: &mut HashMap<&'a str, &'a Value>,
    ) {
        if wanted.is_empty() {
            return;
        }
        match value {
            Value::Array(items) => {
                for item in items {
                    visit(item, wanted, out);
                }
            }
            Value::Object(map) => {
                if let Some(id) = map.get("id").and_then(Value::as_str) {
                    if wanted.remove(id) {
                        out.insert(id, value);
                    }
                }
                if let Some(children) = map.get("children") {
                    visit(children, wanted, out);
                }
            }
            _ => {}
        }
    }
    let mut wanted = ids.into_iter().collect::<HashSet<_>>();
    let mut out = HashMap::new();
    visit(value, &mut wanted, &mut out);
    out
}
/// Preserve node_contains_id's child-array-only semantics. Each requested
/// source subtree is visited at most once, rather than once per root pair.
fn collect_descendant_targets<'a>(
    node: &'a Value,
    targets: &HashSet<&str>,
    out: &mut HashSet<&'a str>,
) {
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            if let Some(id) = child.get("id").and_then(Value::as_str) {
                if targets.contains(id) {
                    out.insert(id);
                }
            }
            collect_descendant_targets(child, targets, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_and_borrows_nested_nodes_without_cloning_subtrees() {
        let forest: Value = serde_json::from_str(
            r##"[{"id":"root","width":1440,"fill":"#123456","children":[{"id":"child","name":"Child"}]}]"##,
        )
        .unwrap();
        let mut ids = HashSet::new();
        index_node_ids(&forest, &mut ids);
        assert_eq!(ids, HashSet::from(["root".into(), "child".into()]));
        let json = chunk_nodes_json(&forest, "[]", &["child".into()]);
        assert!(json.contains("Child"));
        assert!(!json.contains("root"));
        let context = ancestor_context_json(&forest, &["child".into()])
            .expect("nested node has wrapper context");
        assert!(context.contains("root"));
        assert!(context.contains("1440"));
        assert!(context.contains("#123456"));
        assert!(!context.contains("Child"));
    }

    #[test]
    fn ancestor_context_strips_large_metadata_and_default_noise() {
        let forest = serde_json::json!([{
            "id": "root",
            "parentId": "page",
            "pageId": "page",
            "rotation": 0,
            "opacity": 1,
            "visible": true,
            "width": 1440,
            "_meta": { "payload": "x".repeat(MAX_USER_PROMPT_BYTES) },
            "children": [{ "id": "child", "name": "Child" }]
        }]);

        let context = ancestor_context_json(&forest, &["child".into()]).expect("context");
        assert!(
            context.len() < 1_024,
            "unexpected prompt size: {}",
            context.len()
        );
        assert!(context.contains("root"));
        assert!(context.contains("1440"));
        for noise in [
            "parentId", "pageId", "rotation", "opacity", "visible", "_meta",
        ] {
            assert!(!context.contains(noise), "context retained {noise}");
        }
    }

    #[test]
    fn final_output_gate_rejects_prose_but_accepts_each_framework_shape() {
        for framework in Framework::ALL {
            assert!(
                model_output_failure(framework, "Here is the requested implementation.").is_some(),
                "{}",
                framework.as_wire()
            );
        }
        for (framework, code) in [
            (
                Framework::React,
                "export default function App(){ return <main/> }",
            ),
            (Framework::Vue, "<template><main /></template>"),
            (
                Framework::Svelte,
                "<script>let ready = true;</script><main />",
            ),
            (Framework::Html, "<!doctype html><html></html>"),
            (Framework::Flutter, "class App extends StatelessWidget {}"),
            (
                Framework::SwiftUi,
                "struct App: View { var body: some View { Text(\"Hi\") } }",
            ),
            (Framework::Compose, "@Composable fun App() {}"),
            (
                Framework::ReactNative,
                "import { View } from 'react-native';",
            ),
        ] {
            assert_eq!(model_output_failure(framework, code), None, "{framework:?}");
        }
    }

    fn planned(id: &str, dependencies: &[&str]) -> PlannedChunk {
        PlannedChunk {
            id: id.into(),
            name: id.into(),
            node_ids: vec![id.into()],
            role: String::new(),
            suggested_component_name: "Component".into(),
            dependencies: dependencies.iter().map(|value| (*value).into()).collect(),
        }
    }

    #[test]
    fn planner_graph_rejects_duplicate_ids_unknown_dependencies_and_cycles() {
        let duplicate = vec![planned("same", &[]), planned("same", &[])];
        assert!(plan_structure_issue(&duplicate)
            .expect("duplicate id")
            .contains("duplicate chunk id same"));

        let unknown = vec![planned("root", &["missing"])];
        assert!(plan_structure_issue(&unknown)
            .expect("unknown dependency")
            .contains("unknown chunk missing"));

        let cycle = vec![planned("a", &["b"]), planned("b", &["a"])];
        assert!(plan_structure_issue(&cycle)
            .expect("dependency cycle")
            .contains("dependency cycle"));

        let valid = vec![planned("base", &[]), planned("page", &["base"])];
        assert_eq!(plan_structure_issue(&valid), None);
    }
}
