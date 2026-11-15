//! Borrowed JSON-RPC envelope parsing. Only final ToolCall keys and values are
//! owned; params/arguments and unrelated nested fields stay in the input buffer.
//! JSON string decoding uses the original token, without a quoted scratch copy.
use super::{RequestId, ToolCall};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::collections::BTreeMap;

type Object<'a> = BTreeMap<String, &'a RawValue>;

fn object(raw: &str) -> Option<Object<'_>> {
    let mut decoder = serde_json::Deserializer::from_str(raw);
    decoder.disable_recursion_limit();
    let fields = Object::deserialize(&mut decoder).ok()?;
    decoder.end().ok()?;
    Some(fields)
}

fn text(raw: &RawValue) -> Option<String> {
    serde_json::from_str(raw.get()).ok()
}

fn request_id(raw: &RawValue) -> Option<RequestId> {
    if raw.get().starts_with('"') {
        return text(raw).map(RequestId::Str);
    }
    raw.get().parse::<i64>().ok().map(RequestId::Num)
}

/// Accept real MCP tools/call envelopes and legacy direct-method requests.
/// Missing arguments mean an empty map; present non-object arguments fail.
pub fn parse_tool_call(line: &str) -> Option<ToolCall> {
    let envelope = object(line)?;
    let id = request_id(envelope.get("id")?)?;
    let method = text(envelope.get("method")?)?;
    let params = match envelope.get("params") {
        Some(raw) => Some(object(raw.get())?),
        None => None,
    };
    let (tool, arguments) = if method == "tools/call" {
        let params = params?;
        let tool = text(params.get("name")?)?;
        let arguments = match params.get("arguments") {
            Some(raw) => arguments(raw, &tool)?,
            None => BTreeMap::new(),
        };
        (tool, arguments)
    } else {
        let arguments = match params {
            Some(fields) => argument_fields(fields, &method)?,
            None => BTreeMap::new(),
        };
        (method, arguments)
    };
    Some(ToolCall {
        id,
        tool,
        arguments,
    })
}

/// Decode only the top-level id. String ids retain commas, braces, quotes and
/// Unicode escapes correctly; nested id fields cannot shadow the envelope id.
pub fn extract_request_id(line: &str) -> Option<RequestId> {
    request_id(object(line)?.get("id")?)
}

/// Decode one top-level string from a complete catalog descriptor object.
/// Nested schema keys and quoted text cannot shadow name/description fields.
pub(crate) fn extract_string_field(body: &str, key: &str) -> Option<String> {
    text(object(body)?.get(key)?)
}

/// Count one top-level array without allocating its element Value forest.
/// Raw object fields retain the normal last-key-wins duplicate semantics.
pub(crate) fn array_field_len(body: &str, key: &str) -> Option<usize> {
    struct Length(usize);
    impl<'de> Deserialize<'de> for Length {
        fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
            struct Array;
            impl<'de> serde::de::Visitor<'de> for Array {
                type Value = Length;
                fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    formatter.write_str("an array")
                }
                fn visit_seq<A: serde::de::SeqAccess<'de>>(
                    self,
                    mut values: A,
                ) -> Result<Length, A::Error> {
                    let mut count = 0;
                    while values.next_element::<serde::de::IgnoredAny>()?.is_some() {
                        count += 1;
                    }
                    Ok(Length(count))
                }
            }
            decoder.deserialize_seq(Array)
        }
    }
    serde_json::from_str::<Length>(object(body)?.get(key)?.get())
        .ok()
        .map(|length| length.0)
}

fn arguments(raw: &RawValue, tool: &str) -> Option<BTreeMap<String, String>> {
    argument_fields(object(raw.get())?, tool)
}

fn argument_fields(fields: Object<'_>, tool: &str) -> Option<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (key, raw) in fields {
        let value = match raw.get().as_bytes().first()? {
            b'"' => text(raw)?,
            b'{' | b'[' if !structured_arg_allowed(tool, &key) => return None,
            _ => raw.get().to_owned(),
        };
        out.insert(key, value);
    }
    Some(out)
}

fn structured_arg_allowed(tool: &str, key: &str) -> bool {
    if tool.starts_with("add_") {
        return !key.is_empty();
    }
    matches!(
        (tool, key),
        ("set_node_layout_property", "value")
            | ("batch_tools", "operations")
            | ("set_variables", "variables")
            | ("upsert_variables", "variables")
            | ("upsert_component", "node_json")
            | ("upsert_screen", "node_json")
            | ("set_themes", "themes")
            | ("read_nodes", "nodeIds")
            | ("export_nodes", "nodeIds")
            // The lean MCP profile's get_editor_state forwards nodeIds to
            // read_nodes; the full-catalog tool ignores arguments.
            | ("get_editor_state", "nodeIds")
            | ("batch_get", "patterns")
            | ("batch_get", "nodeIds")
            | ("search_all_unique_properties", "parents")
            | ("search_all_unique_properties", "properties")
            | ("replace_all_matching_properties", "parents")
            | ("replace_all_matching_properties", "properties")
            | ("codegen_plan", "plan")
            | ("codegen_submit_chunk", "result")
            | ("copy_node", "overrides")
            | ("insert_node", "data")
            | ("update_node", "data")
            | ("replace_node", "data")
            | ("design_skeleton", "rootFrame")
            | ("design_skeleton", "sections")
            | ("design_skeleton", "styleGuide")
            | ("design_content", "children")
            | ("get_style_guide", "tags")
            | ("finalize_design", "root_ids")
            | ("enrich_images", "root_ids")
    )
}
