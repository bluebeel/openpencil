//! Encode one requested node at a time instead of retaining a second complete
//! JSON forest beside the final reply. Keep the existing snapshot codec so
//! depth truncation, float conversion and resource representations stay exact.
use jian_ops_schema::node::PenNode;
use serde_json::Value;

pub(crate) fn encode<'a>(
    nodes: impl IntoIterator<Item = &'a PenNode>,
    depth: i32,
    mut transform: impl FnMut(&mut Value),
    metadata: impl FnOnce() -> Option<(Value, Value)>,
) -> Result<String, serde_json::Error> {
    let mut output = b"{\"nodes\":".to_vec();
    append_nodes(&mut output, nodes, depth, &mut transform)?;
    if let Some((variables, themes)) = metadata() {
        // Preserve the existing serde_json::Map feature-dependent field order
        // without putting the potentially enormous node array into that map.
        let mut fields = serde_json::Map::new();
        fields.insert("variables".into(), variables);
        fields.insert("themes".into(), themes);
        for (key, value) in fields {
            output.push(b',');
            serde_json::to_writer(&mut output, &key)?;
            output.push(b':');
            serde_json::to_writer(&mut output, &value)?;
        }
    }
    output.push(b'}');
    String::from_utf8(output).map_err(|error| {
        serde_json::Error::io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })
}

/// Emit a small metadata object around a streamed node array. The caller adds a
/// `nodes` placeholder in its original insertion position, so both sorted and
/// preserve_order Map configurations keep their existing wire order.
pub(crate) fn encode_with_fields<'a>(
    nodes: impl IntoIterator<Item = &'a PenNode>,
    depth: i32,
    fields: serde_json::Map<String, Value>,
) -> Result<String, serde_json::Error> {
    let mut output = vec![b'{'];
    let mut nodes = Some(nodes);
    for (index, (key, value)) in fields.into_iter().enumerate() {
        if index > 0 {
            output.push(b',');
        }
        serde_json::to_writer(&mut output, &key)?;
        output.push(b':');
        if key == "nodes" {
            if let Some(nodes) = nodes.take() {
                append_nodes(&mut output, nodes, depth, &mut |_| {})?;
            }
        } else {
            serde_json::to_writer(&mut output, &value)?;
        }
    }
    output.push(b'}');
    String::from_utf8(output).map_err(|error| {
        serde_json::Error::io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })
}

fn append_nodes<'a>(
    output: &mut Vec<u8>,
    nodes: impl IntoIterator<Item = &'a PenNode>,
    depth: i32,
    transform: &mut impl FnMut(&mut Value),
) -> Result<(), serde_json::Error> {
    output.push(b'[');
    for (index, node) in nodes.into_iter().enumerate() {
        if index > 0 {
            output.push(b',');
        }
        let mut value = super::node_snapshot_value(node, depth);
        transform(&mut value);
        serde_json::to_writer(&mut *output, &value)?;
    }
    output.push(b']');
    Ok(())
}
