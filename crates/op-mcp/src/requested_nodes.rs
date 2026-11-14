//! Borrowed multi-ID lookup in one request-local tree traversal.

use jian_ops_schema::node::PenNode;
use op_editor_core::{walkers::find_node, NodeId, PenNodeExt};
use std::collections::{HashMap, HashSet};

/// First DFS match for each requested ID, in request order including duplicates.
/// No cross-call cache: public documents may change without a revision bump.
pub(super) fn find<'a, S: AsRef<str>>(roots: &'a [PenNode], ids: &[S]) -> Vec<&'a PenNode> {
    if ids.len() == 1 {
        // Keep the common single-ID early-exit path without an index/set.
        return NodeId::new_opt(ids[0].as_ref())
            .and_then(|id| find_node(roots, &id))
            .into_iter()
            .collect();
    }
    find_multiple(roots, ids, &mut || {})
}

/// Reuse one proof-bearing locked-host index. Public snapshots retain the
/// request-local traversal, including raw first-DFS/missing/duplicate behavior.
pub(super) fn find_for_state<'a, S: AsRef<str>>(
    state: &'a op_editor_core::EditorState,
    roots: &'a [PenNode],
    ids: &[S],
) -> Vec<&'a PenNode> {
    if ids.is_empty() {
        return Vec::new();
    }
    if let Some(lookup) = state
        .trusted_node_lookup(roots)
        .filter(|lookup| lookup.is_indexed())
    {
        return ids
            .iter()
            .filter_map(|id| {
                let id = id.as_ref();
                (!id.is_empty())
                    .then(|| lookup.find(id, op_editor_core::NodeReadOrder::First))
                    .flatten()
                    .map(|found| found.node)
            })
            .collect();
    }
    find(roots, ids)
}

/// Complete ordered forests retain first-page precedence. A missing ID does
/// not trigger a second fallback tree scan while the index proves its absence.
pub(super) fn find_in_forests_for_state<'a, S: AsRef<str>>(
    state: &'a op_editor_core::EditorState,
    forests: impl IntoIterator<Item = &'a [PenNode]>,
    ids: &[S],
) -> Vec<&'a PenNode> {
    if ids.is_empty() {
        return Vec::new();
    }
    let forests: Vec<_> = forests.into_iter().collect();
    let lookups: Option<Vec<_>> = forests
        .iter()
        .map(|&roots| state.trusted_node_lookup(roots))
        .collect();
    if let Some(lookups) =
        lookups.filter(|lookups| lookups.iter().all(|lookup| lookup.is_indexed()))
    {
        return ids
            .iter()
            .filter_map(|id| {
                let id = id.as_ref();
                if id.is_empty() {
                    return None;
                }
                lookups.iter().find_map(|lookup| {
                    lookup
                        .find(id, op_editor_core::NodeReadOrder::First)
                        .map(|found| found.node)
                })
            })
            .collect();
    }
    find_in_forests(forests, ids)
}

/// First matches across ordered document forests. Keep one unresolved-ID set
/// across pages and stop as soon as all targets have their first match.
pub(super) fn find_in_forests<'a, S: AsRef<str>>(
    forests: impl IntoIterator<Item = &'a [PenNode]>,
    ids: &[S],
) -> Vec<&'a PenNode> {
    if ids.len() == 1 {
        let Some(id) = NodeId::new_opt(ids[0].as_ref()) else {
            return Vec::new();
        };
        for roots in forests {
            if let Some(node) = find_node(roots, &id) {
                return vec![node];
            }
        }
        return Vec::new();
    }
    find_multiple_forests(forests, ids, &mut || {})
}

fn find_multiple<'a, S: AsRef<str>>(
    roots: &'a [PenNode],
    ids: &[S],
    visited: &mut impl FnMut(),
) -> Vec<&'a PenNode> {
    find_multiple_forests(std::iter::once(roots), ids, visited)
}

fn find_multiple_forests<'a, S: AsRef<str>>(
    forests: impl IntoIterator<Item = &'a [PenNode]>,
    ids: &[S],
    visited: &mut impl FnMut(),
) -> Vec<&'a PenNode> {
    // Empty is exactly NodeId::NONE; arbitrary nonempty authored IDs are valid.
    let mut wanted: HashSet<&str> = ids
        .iter()
        .map(AsRef::as_ref)
        .filter(|id| !id.is_empty())
        .collect();
    let mut found = HashMap::with_capacity(wanted.len());
    for roots in forests {
        if wanted.is_empty() {
            break;
        }
        visit(roots, &mut wanted, &mut found, visited);
    }
    ids.iter()
        .filter_map(|id| found.get(id.as_ref()).copied())
        .collect()
}

fn visit<'a>(
    roots: &'a [PenNode],
    wanted: &mut HashSet<&str>,
    found: &mut HashMap<&'a str, &'a PenNode>,
    visited: &mut impl FnMut(),
) {
    for node in roots {
        if wanted.is_empty() {
            return;
        }
        visited();
        if wanted.remove(node.id_str()) {
            found.insert(node.id_str(), node);
        }
        if let Some(children) = node.children() {
            visit(children, wanted, found, visited);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{batch_get_snapshot_for_call, read_nodes_snapshot_for_call, McpTool, ToolOutcome};
    use op_editor_core::EditorState;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;

    fn state() -> EditorState {
        EditorState::from_document(
            serde_json::from_value(json!({"version":"1.0.0","children":[
                {"type":"frame","id":"p","children":[
                    {"type":"rectangle","id":"a","name":"first"},
                    {"type":"rectangle","id":"b","name":"B"}]},
                {"type":"frame","id":"q","children":[
                    {"type":"rectangle","id":"a","name":"second"},
                    {"type":"rectangle","id":"c","name":"C"}]}
            ]}))
            .unwrap(),
        )
    }
    fn value(outcome: ToolOutcome) -> Value {
        match outcome {
            ToolOutcome::OkJson(value) => serde_json::from_str(&value).unwrap(),
            other => panic!("{other:?}"),
        }
    }
    #[test]
    fn multi_id_lookup_keeps_legacy_first_match_order_duplicates_and_missing_ids() {
        let state = state();
        let ids = ["c", "a", "missing", "", "a", "b"];
        let expected: Vec<_> = ids
            .iter()
            .filter_map(|id| {
                NodeId::new_opt(*id).and_then(|id| find_node(state.active_children(), &id))
            })
            .collect();
        let actual = find(state.active_children(), &ids);
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(expected) {
            assert!(std::ptr::eq(*actual, expected));
        }
        assert_eq!(
            find(state.active_children(), &["a"])[0]
                .base()
                .name
                .as_deref(),
            Some("first")
        );
        assert!(find(state.active_children(), &[""]).is_empty());
    }
    #[test]
    fn multi_id_search_visits_each_node_once_and_stops_after_last_requested_match() {
        let state = state();
        let mut visits = 0;
        let actual = find_multiple(state.active_children(), &["a", "b", "a"], &mut || {
            visits += 1
        });
        assert_eq!(actual.len(), 3);
        assert_eq!(
            visits, 3,
            "only p/a/b; later q and descendants are not visited"
        );
        visits = 0;
        find_multiple(state.active_children(), &["a", "missing", "b"], &mut || {
            visits += 1
        });
        assert_eq!(
            visits, 6,
            "a miss scans six nodes once, not once per requested ID"
        );
    }
    #[test]
    fn read_nodes_repeats_requested_duplicates_while_batch_get_keeps_pattern_precedence() {
        let state = state();
        let read = BTreeMap::from([
            ("nodeIds".into(), r#"["c","a","a",""]"#.into()),
            ("depth".into(), "0".into()),
        ]);
        let result = value(read_nodes_snapshot_for_call(&state, &read).call(&read));
        assert_eq!(
            result["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["c", "a", "a"]
        );
        let batch = BTreeMap::from([
            (
                "patterns".into(),
                r#"[{"name":"^B$"},{"name":"^first$"}]"#.into(),
            ),
            ("nodeIds".into(), r#"["a","c","c","missing"]"#.into()),
            ("readDepth".into(), "0".into()),
        ]);
        let result = value(batch_get_snapshot_for_call(&state, &batch).call(&batch));
        assert_eq!(
            result["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["b", "a", "c"]
        );
    }
    #[test]
    fn batch_get_parent_filters_patterns_but_explicit_ids_still_search_whole_page() {
        let state = state();
        let args = BTreeMap::from([
            ("parentId".into(), "q".into()),
            ("patterns".into(), r#"[{"name":"^second$"}]"#.into()),
            ("nodeIds".into(), r#"["a","b"]"#.into()),
            ("readDepth".into(), "0".into()),
        ]);
        let result = value(batch_get_snapshot_for_call(&state, &args).call(&args));
        assert_eq!(result["nodes"][0]["name"], "second");
        assert_eq!(result["nodes"][1]["id"], "b");
        assert_eq!(result["nodes"].as_array().unwrap().len(), 2);
    }

    #[test]
    #[ignore = "Explicit cached-binary stage benchmark; needs immutable document environment"]
    fn multi_id_stage_benchmark() {
        use std::{hint::black_box, io::BufReader, time::Instant};
        let path = std::env::var_os("OPENPENCIL_QUERY_BENCH_DOCUMENT")
            .expect("caller must pin/hash an immutable full document");
        let document: jian_ops_schema::PenDocument = serde_json::from_reader(BufReader::new(
            std::fs::File::open(&path).expect("open benchmark document"),
        ))
        .expect("typed complete document");
        fn flatten<'a>(roots: &'a [PenNode], nodes: &mut Vec<&'a PenNode>) {
            for node in roots {
                nodes.push(node);
                if let Some(children) = node.children() {
                    flatten(children, nodes);
                }
            }
        }
        let mut chosen = ("0", document.children.as_slice(), Vec::new());
        flatten(chosen.1, &mut chosen.2);
        for page in document.pages.as_deref().unwrap_or_default() {
            let mut nodes = Vec::new();
            flatten(&page.children, &mut nodes);
            if nodes.len() > chosen.2.len() {
                chosen = (page.id.as_str(), &page.children, nodes);
            }
        }
        assert!(!chosen.2.is_empty());
        let mut results = Vec::new();
        for requested in [1, 10, 100, 1000] {
            let count = requested.min(chosen.2.len());
            let ids: Vec<String> = (0..count)
                .map(|i| {
                    let index = if count == 1 {
                        chosen.2.len() - 1
                    } else {
                        i * (chosen.2.len() - 1) / (count - 1)
                    };
                    chosen.2[index].id_str().to_owned()
                })
                .collect();
            let legacy = || {
                ids.iter()
                    .filter_map(|id| NodeId::new_opt(id).and_then(|id| find_node(chosen.1, &id)))
                    .collect::<Vec<_>>()
            };
            let expected = legacy();
            let actual = find(chosen.1, &ids);
            assert_eq!(expected.len(), actual.len());
            assert!(expected
                .iter()
                .zip(&actual)
                .all(|(a, b)| std::ptr::eq(*a, *b)));
            let (mut old_seconds, mut new_seconds) = (0.0, 0.0);
            for round in 0..4 {
                // Alternate order in the same executable with the same input.
                for old in if round % 2 == 0 {
                    [true, false]
                } else {
                    [false, true]
                } {
                    let started = Instant::now();
                    if old {
                        black_box(legacy());
                    } else {
                        black_box(find(chosen.1, &ids));
                    }
                    let elapsed = started.elapsed().as_secs_f64();
                    if old {
                        old_seconds += elapsed;
                    } else {
                        new_seconds += elapsed;
                    }
                }
            }
            results.push(
                json!({"requestedIds":requested,"actualIds":count,"roundsPerMethod":4,
                "legacyMeanSeconds":old_seconds/4.0,"singleTraversalMeanSeconds":new_seconds/4.0,
                "ratio":old_seconds/new_seconds,"orderedFirstMatchIdentity":true}),
            );
        }
        eprintln!(
            "QUERY_STAGE_BENCHMARK={}",
            json!({"pageId":chosen.0,
            "pageNodes":chosen.2.len(),"cases":results,
            "scope":"Borrowed selector stage only, same emitted test binary/profile. Excludes input load, JSON serialization, RPC, history and persistence; not an agent throughput or service-memory result."})
        );
    }
}
