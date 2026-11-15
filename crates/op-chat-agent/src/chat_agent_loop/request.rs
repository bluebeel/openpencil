//! Borrow immutable transcript/schema trees during request encoding.

use op_ai::chat_provider::ChatToolDef;
use serde::Serialize;
use serde_json::{Map, Value};

/// Reasoning policy edits only `controls`; messages/tools keep one owner for
/// the entire run instead of being cloned into another Value every turn.
#[derive(Serialize)]
pub(super) struct AgentRequest<'a> {
    #[serde(flatten)]
    pub controls: &'a Value,
    pub messages: &'a [Value],
    pub tools: &'a [Value],
}

pub(super) fn tool_definitions(tools: &[ChatToolDef], openai: bool) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            let schema = serde_json::from_str::<Value>(&tool.input_schema_json)
                .unwrap_or_else(|_| serde_json::json!({"type":"object"}));
            let mut definition = Map::new();
            definition.insert("name".into(), Value::String(tool.name.clone()));
            definition.insert(
                "description".into(),
                Value::String(tool.description.clone()),
            );
            definition.insert(
                if openai { "parameters" } else { "input_schema" }.into(),
                schema,
            );
            if !openai {
                return Value::Object(definition);
            }
            let mut outer = Map::new();
            outer.insert("type".into(), Value::String("function".into()));
            outer.insert("function".into(), Value::Object(definition));
            Value::Object(outer)
        })
        .collect()
}
