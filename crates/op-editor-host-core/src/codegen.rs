//! Shared code-generation input builders.

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};

use jian_ops_schema::node::PenNode;
use op_ai::chat_provider::{EffortLevel, ThinkingMode};
use op_codegen::ai::types::CodegenInput;
use op_editor_core::state::EditorState;
use op_editor_core::PenNodeExt;

/// Default hard per-request token cap. Phase builders stay within it while
/// selecting planning/chunk/assembly budgets of 6k/12k/16k respectively.
pub const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 16_000;

/// A bounded live-export serialization failed before returning a JSON copy.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum CodegenNodesJsonError {
    #[error("Codegen nodes JSON is too large ({input_bytes} bytes observed; maximum is {max_bytes} bytes)")]
    TooLarge {
        input_bytes: usize,
        max_bytes: usize,
    },
    #[error("Could not serialize codegen nodes JSON: {message}")]
    Serialization { message: String },
}

/// Build pipeline input from the current selection, falling back to the
/// active page's children when nothing is selected. Returns the input plus
/// the raw selected-nodes JSON used by desktop export bundling.
pub fn build_codegen_input(state: &EditorState) -> Option<(CodegenInput, String)> {
    let selected = codegen_target_nodes(state)?;
    let input = input_from_nodes(&selected, state.codegen.framework, &state.doc);
    let raw_nodes_json = input.nodes_json.clone();
    Some((input, raw_nodes_json))
}

/// Serialize only the selected/page nodes for live bundle export, stopping
/// before the output can exceed `max_bytes`. Unlike `build_codegen_input`,
/// this returns exactly one owned JSON buffer and never serializes variables.
pub(crate) fn build_codegen_nodes_json_limited(
    state: &EditorState,
    max_bytes: usize,
) -> Result<Option<String>, CodegenNodesJsonError> {
    let Some(nodes) = codegen_target_nodes(state) else {
        return Ok(None);
    };
    let mut writer = LimitedJsonWriter::new(max_bytes);
    let result = serde_json::to_writer(&mut writer, &nodes);
    if let Some(input_bytes) = writer.overflow_at {
        return Err(CodegenNodesJsonError::TooLarge {
            input_bytes,
            max_bytes,
        });
    }
    result.map_err(|error| CodegenNodesJsonError::Serialization {
        message: error.to_string(),
    })?;
    String::from_utf8(writer.bytes).map(Some).map_err(|error| {
        CodegenNodesJsonError::Serialization {
            message: error.utf8_error().to_string(),
        }
    })
}

/// Web hosts do not need the raw selected-nodes JSON because their bundle is
/// assembled from live selection state.
pub fn build_codegen_input_value(state: &EditorState) -> Option<CodegenInput> {
    let selected = codegen_target_nodes(state)?;
    Some(input_from_nodes(
        &selected,
        state.codegen.framework,
        &state.doc,
    ))
}

fn codegen_target_nodes(state: &EditorState) -> Option<Vec<&PenNode>> {
    if state.selection.is_empty() {
        let children = state.active_children();
        return (!children.is_empty()).then(|| children.iter().collect());
    }

    // Find all requested IDs in one canonical preorder walk, retaining the
    // first page/preorder match just as the prior per-ID searches did. The
    // final projection retains selection iteration order and overlapping roots.
    let mut remaining: HashSet<&str> = state.selection.set.iter().map(|id| id.as_str()).collect();
    let mut found = HashMap::with_capacity(remaining.len());
    let mut stack = Vec::new();
    if let Some(pages) = state.doc.pages.as_ref() {
        for page in pages {
            collect_codegen_targets(&page.children, &mut remaining, &mut found, &mut stack);
            if remaining.is_empty() {
                break;
            }
        }
    } else {
        collect_codegen_targets(&state.doc.children, &mut remaining, &mut found, &mut stack);
    }
    let selected: Vec<_> = state
        .selection
        .set
        .iter()
        .filter_map(|id| found.get(id.as_str()).copied())
        .collect();
    (!selected.is_empty()).then_some(selected)
}

fn collect_codegen_targets<'a>(
    roots: &'a [PenNode],
    remaining: &mut HashSet<&str>,
    found: &mut HashMap<&'a str, &'a PenNode>,
    stack: &mut Vec<std::slice::Iter<'a, PenNode>>,
) {
    stack.push(roots.iter());
    while !remaining.is_empty() {
        let Some(siblings) = stack.last_mut() else {
            break;
        };
        let Some(node) = siblings.next() else {
            stack.pop();
            continue;
        };
        if remaining.remove(node.id_str()) {
            found.insert(node.id_str(), node);
        }
        if let Some(children) = node.children().filter(|children| !children.is_empty()) {
            stack.push(children.iter());
        }
    }
    stack.clear();
}

struct LimitedJsonWriter {
    bytes: Vec<u8>,
    max_bytes: usize,
    overflow_at: Option<usize>,
}

impl LimitedJsonWriter {
    fn new(max_bytes: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(max_bytes.min(64 * 1024)),
            max_bytes,
            overflow_at: None,
        }
    }
}

impl Write for LimitedJsonWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let input_bytes = self.bytes.len().saturating_add(buffer.len());
        if input_bytes > self.max_bytes {
            self.overflow_at.get_or_insert(input_bytes);
            return Err(io::Error::other("codegen nodes JSON limit exceeded"));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn input_from_nodes(
    nodes: &[&PenNode],
    framework: op_editor_core::codegen::Framework,
    doc: &jian_ops_schema::PenDocument,
) -> CodegenInput {
    let nodes_json = serde_json::to_string(nodes).unwrap_or_else(|_| "[]".to_string());
    let variables_json = doc
        .variables
        .as_ref()
        .filter(|vars| !vars.is_empty())
        .map(|vars| serde_json::to_string(vars).unwrap_or_else(|_| "{}".to_string()));
    let themes_json = doc
        .themes
        .as_ref()
        .filter(|themes| !themes.is_empty())
        .and_then(|themes| serde_json::to_string(themes).ok());
    // Masters of instances in the selection that were not themselves
    // selected, so instance-aware generators can resolve them.
    let components = op_codegen::referenced_components(nodes, doc);
    let components_json = (!components.is_empty())
        .then(|| serde_json::to_string(&components).ok())
        .flatten();

    CodegenInput {
        nodes_json,
        framework,
        variables_json,
        themes_json,
        components_json,
        max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
        thinking: ThinkingMode::Adaptive,
        effort: EffortLevel::Low,
    }
}

/// File extension for the active framework's generated component file.
pub fn framework_ext(fw: op_editor_core::codegen::Framework) -> &'static str {
    use op_editor_core::codegen::Framework;
    match fw {
        Framework::React | Framework::ReactTailwind | Framework::ReactNative => "tsx",
        Framework::Vue => "vue",
        Framework::Svelte => "svelte",
        Framework::Html => "html",
        Framework::Flutter => "dart",
        Framework::SwiftUi => "swift",
        Framework::Compose => "kt",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> EditorState {
        let doc = jian_ops_schema::load_str(
            r#"{"version":"1.0.0","children":[
                {"type":"rectangle","id":"n1","name":"A","width":10,"height":10},
                {"type":"rectangle","id":"n2","name":"B","width":10,"height":10}
            ]}"#,
        )
        .expect("fixture parses")
        .value;
        EditorState::from_document(doc)
    }

    #[test]
    fn limited_json_matches_generation_raw_json_without_a_second_copy() {
        let mut state = state();
        state.set_single_selection(op_editor_core::NodeId::new("n1"));
        let (_, generation_raw) = build_codegen_input(&state).expect("generation input");
        let limited = build_codegen_nodes_json_limited(&state, generation_raw.len())
            .expect("bounded serialization")
            .expect("live nodes");
        assert_eq!(limited, generation_raw);
    }

    #[test]
    fn limited_json_returns_a_typed_error_as_soon_as_the_cap_is_crossed() {
        let error = build_codegen_nodes_json_limited(&state(), 32)
            .expect_err("fixture exceeds the tiny cap");
        assert!(matches!(
            error,
            CodegenNodesJsonError::TooLarge {
                input_bytes,
                max_bytes: 32,
            } if input_bytes > 32
        ));
    }

    #[test]
    fn limited_writer_never_appends_the_over_limit_chunk() {
        let mut writer = LimitedJsonWriter::new(4);
        writer.write_all(b"1234").expect("at limit");
        assert!(writer.write_all(b"5").is_err());
        assert_eq!(writer.bytes, b"1234");
        assert_eq!(writer.overflow_at, Some(5));
    }
}
