use std::collections::BTreeMap;

use op_mcp::ToolCall;
use serde_json::{Map, Value};

use super::{BatchError, Result};

const MAX_INPUT_BYTES: usize = 1024 * 1024;
const MAX_TOTAL_ARGUMENT_BYTES: usize = 8 * 1024 * 1024;

pub(super) struct Step {
    pub name: String,
    pub arguments: Value,
    pub bind: Option<String>,
}

pub(super) fn parse(arguments: &BTreeMap<String, String>) -> Result<Vec<Step>> {
    if arguments
        .keys()
        .any(|name| !matches!(name.as_str(), "operations" | "agentId" | "filePath"))
    {
        return Err(BatchError::argument(
            "only operations, outer agentId and outer filePath are supported",
        ));
    }
    let raw = arguments
        .get("operations")
        .ok_or_else(|| BatchError::argument("operations is required"))?;
    if raw.len() > MAX_INPUT_BYTES {
        return Err(BatchError::argument("operations exceeds 1 MiB"));
    }
    let value: Value = serde_json::from_str(raw)
        .map_err(|e| BatchError::argument(format!("operations must be valid JSON: {e}")))?;
    let Value::Array(items) = value else {
        return Err(BatchError::argument("operations must be an array"));
    };
    if items.is_empty() || items.len() > 128 {
        return Err(BatchError::argument("operations must contain 1..128 steps"));
    }
    let mut bindings = std::collections::HashSet::new();
    items.into_iter().enumerate().map(|(index, item)| {
        let Value::Object(mut item) = item else {
            return Err(BatchError::argument("operation must be an object").at(index));
        };
        if item.keys().any(|key| !matches!(key.as_str(), "name" | "arguments" | "bind")) {
            return Err(BatchError::argument("operation accepts only name, arguments and bind").at(index));
        }
        let name = match item.remove("name") {
            Some(Value::String(name)) if !name.is_empty() && name.len() <= 128 => name,
            _ => return Err(BatchError::argument("operation name must contain 1..128 bytes").at(index)),
        };
        let arguments = item.remove("arguments").unwrap_or_else(|| Value::Object(Map::new()));
        if !arguments.is_object() {
            return Err(BatchError::argument("operation arguments must be an object").at(index));
        }
        let bind = match item.remove("bind") {
            None => None,
            Some(Value::String(bind)) if valid_binding(&bind) && bindings.insert(bind.clone()) => Some(bind),
            _ => return Err(BatchError::argument("bind must be unique, start with an ASCII letter and contain 1..64 letters/digits/_/-").at(index)),
        };
        Ok(Step { name, arguments, bind })
    }).collect()
}

fn valid_binding(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.as_bytes()[0].is_ascii_alphabetic()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

pub(super) fn resolve(
    value: Value,
    results: &[Value],
    bindings: &BTreeMap<String, usize>,
) -> Result<Value> {
    resolve_at(value, results, bindings, 0)
}

fn resolve_at(
    value: Value,
    results: &[Value],
    bindings: &BTreeMap<String, usize>,
    depth: usize,
) -> Result<Value> {
    if depth > 64 {
        return Err(BatchError::argument(
            "argument/reference nesting exceeds 64",
        ));
    }
    match value {
        Value::Object(mut object) => {
            // Escapes the reserved reference syntax without interpreting any
            // object inside the literal payload.
            if object.len() == 1 && object.contains_key("$literal") {
                return Ok(object.remove("$literal").unwrap());
            }
            if object.len() == 1 && object.contains_key("$ref") {
                let Some(Value::String(reference)) = object.remove("$ref") else {
                    return Err(BatchError::argument("$ref must be a string"));
                };
                let Some((source, pointer)) = reference.split_once('#') else {
                    return Err(BatchError::argument(
                        "$ref must be priorBind#/json/pointer or priorIndex#/json/pointer",
                    ));
                };
                let index = bindings
                    .get(source)
                    .copied()
                    .or_else(|| source.parse::<usize>().ok())
                    .filter(|index| *index < results.len())
                    .ok_or_else(|| {
                        BatchError::argument("$ref must name a completed earlier operation")
                    })?;
                return results[index]
                    .pointer(pointer)
                    .cloned()
                    .ok_or_else(|| BatchError::argument("$ref JSON pointer does not resolve"));
            }
            let mut output = Map::new();
            for (key, value) in object {
                output.insert(key, resolve_at(value, results, bindings, depth + 1)?);
            }
            Ok(Value::Object(output))
        }
        Value::Array(values) => values
            .into_iter()
            .map(|value| resolve_at(value, results, bindings, depth + 1))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array),
        value => Ok(value),
    }
}

pub(super) fn call(
    index: usize,
    name: &str,
    arguments: Value,
    total_bytes: &mut usize,
) -> Result<ToolCall> {
    let request = serde_json::json!({"id":index,"method":"tools/call",
        "params":{"name":name,"arguments":arguments}})
    .to_string();
    *total_bytes = total_bytes.saturating_add(request.len());
    if request.len() > MAX_INPUT_BYTES || *total_bytes > MAX_TOTAL_ARGUMENT_BYTES {
        return Err(BatchError::argument(
            "resolved arguments exceed 1 MiB per step or 8 MiB per batch",
        ));
    }
    // Reuse the real wire argument parser, including each subtool's structured
    // argument allowlist and exact float-roundtrip serialization.
    op_mcp::parse_tool_call(&request).ok_or_else(|| {
        BatchError::argument("subtool arguments do not satisfy its MCP wire contract")
    })
}

pub(super) fn one_authority(arguments: &BTreeMap<String, String>) -> Result<()> {
    if arguments.keys().any(|key| {
        matches!(
            key.as_str(),
            "agentId" | "filePath" | "file_path" | "sourceFilePath" | "source_file_path"
        )
    }) {
        return Err(BatchError::argument("subcalls cannot change agent or document authority; use only the outer agentId/filePath"));
    }
    Ok(())
}
