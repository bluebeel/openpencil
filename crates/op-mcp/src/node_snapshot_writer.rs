//! Depth-projected snapshots retain one header per open ancestor, not an
//! entire Value forest. Header fields still use the original Value serializer,
//! retaining exact numeric conversion and feature-dependent object key order.
use jian_ops_schema::node::PenNode;
use serde_json::Value;

enum Task<'a> {
    Node(&'a PenNode, i32),
    Fields {
        fields: serde_json::map::IntoIter,
        children: Option<&'a [PenNode]>,
        depth: i32,
        first: bool,
    },
    Children {
        nodes: std::slice::Iter<'a, PenNode>,
        depth: i32,
        first: bool,
    },
}

pub(crate) fn write_snapshot(
    output: &mut Vec<u8>,
    root: &PenNode,
    depth: i32,
    transform: &mut impl FnMut(&mut Value),
) -> Result<(), serde_json::Error> {
    let mut tasks = vec![Task::Node(root, depth)];
    while let Some(task) = tasks.pop() {
        match task {
            Task::Node(node, depth) => {
                let mut value = super::header_value(node, depth)?;
                // Current internal transforms resolve this node's own metadata;
                // descendants are handled independently, with empty placeholders.
                transform(&mut value);
                if let Value::Object(fields) = value {
                    output.push(b'{');
                    tasks.push(Task::Fields {
                        fields: fields.into_iter(),
                        children: node.children_ref(),
                        depth,
                        first: true,
                    });
                } else {
                    serde_json::to_writer(&mut *output, &value)?;
                }
            }
            Task::Fields {
                mut fields,
                children,
                depth,
                first,
            } => {
                if let Some((key, value)) = fields.next() {
                    if !first {
                        output.push(b',');
                    }
                    serde_json::to_writer(&mut *output, &key)?;
                    output.push(b':');
                    let descendants = (key == "children" && (depth == -1 || depth > 0))
                        .then_some(children)
                        .flatten()
                        .filter(|nodes| !nodes.is_empty());
                    tasks.push(Task::Fields {
                        fields,
                        children,
                        depth,
                        first: false,
                    });
                    if let Some(nodes) = descendants {
                        output.push(b'[');
                        tasks.push(Task::Children {
                            nodes: nodes.iter(),
                            depth: if depth == -1 { -1 } else { depth - 1 },
                            first: true,
                        });
                    } else {
                        serde_json::to_writer(&mut *output, &value)?;
                    }
                } else {
                    output.push(b'}');
                }
            }
            Task::Children {
                mut nodes,
                depth,
                first,
            } => {
                if let Some(node) = nodes.next() {
                    if !first {
                        output.push(b',');
                    }
                    tasks.push(Task::Children {
                        nodes,
                        depth,
                        first: false,
                    });
                    tasks.push(Task::Node(node, depth));
                } else {
                    output.push(b']');
                }
            }
        }
    }
    Ok(())
}
