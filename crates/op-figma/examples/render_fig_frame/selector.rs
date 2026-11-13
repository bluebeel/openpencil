use super::selector_error::SelectorError;
use jian_ops_schema::node::PenNode;
use op_editor_core::PenNodeExt;
use serde_json::{json, Value};

#[derive(Debug)]
pub struct Segment {
    name: String,
    index: Option<usize>,
}
#[derive(Debug)]
pub enum Selector {
    Path(Vec<Segment>),
    Id(String),
}
pub struct Selection<'a> {
    pub node: &'a PenNode,
    pub path: Vec<Value>,
}

impl Selector {
    pub fn top_level(name: String) -> Self {
        Self::Path(vec![Segment { name, index: None }])
    }
    pub fn path_json(source: &str) -> Result<Self, SelectorError> {
        let value: Value = serde_json::from_str(source)?;
        let values = value
            .as_array()
            .filter(|v| !v.is_empty())
            .ok_or(SelectorError::PathArrayRequired)?;
        let mut path = Vec::new();
        for (at, value) in values.iter().enumerate() {
            path.push(if let Some(name) = value.as_str() {
                Segment {
                    name: name.into(),
                    index: None,
                }
            } else {
                let object = value.as_object().ok_or(SelectorError::SegmentType { at })?;
                if let Some(field) = object.keys().find(|key| *key != "name" && *key != "index") {
                    return Err(SelectorError::UnknownField {
                        at,
                        field: field.clone(),
                    });
                }
                let name = value
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or(SelectorError::MissingName { at })?;
                let index = value
                    .get("index")
                    .map(|v| {
                        v.as_u64()
                            .and_then(|i| usize::try_from(i).ok())
                            .ok_or_else(|| SelectorError::InvalidIndex {
                                at,
                                value: v.clone(),
                            })
                    })
                    .transpose()?;
                Segment {
                    name: name.into(),
                    index,
                }
            });
        }
        Ok(Self::Path(path))
    }
}
fn identity(node: &PenNode, index: usize) -> Value {
    json!({"id":node.base().id,"name":node.base().name,"index":index})
}

pub fn select<'a>(
    roots: &'a [PenNode],
    selector: &Selector,
) -> Result<Selection<'a>, SelectorError> {
    match selector {
        Selector::Path(segments) => {
            let mut siblings = roots;
            let mut path = Vec::new();
            let mut selected = None;
            for (at, segment) in segments.iter().enumerate() {
                let matches: Vec<_> = siblings
                    .iter()
                    .filter(|node| node.base().name.as_deref().unwrap_or("") == segment.name)
                    .collect();
                let index = match segment.index {
                    Some(index) if index < matches.len() => index,
                    Some(index) => {
                        return Err(SelectorError::IndexOutOfRange {
                            at,
                            name: segment.name.clone(),
                            index,
                            matches: matches.len(),
                        });
                    }
                    None if matches.len() == 1 => 0,
                    None => {
                        return Err(SelectorError::PathMatchCount {
                            at,
                            name: segment.name.clone(),
                            matches: matches.len(),
                        });
                    }
                };
                let node = matches[index];
                path.push(identity(node, index));
                selected = Some(node);
                siblings = node.children().map(Vec::as_slice).unwrap_or(&[]);
            }
            Ok(Selection {
                node: selected.ok_or(SelectorError::EmptyPath)?,
                path,
            })
        }
        Selector::Id(id) => {
            fn visit<'a>(
                nodes: &'a [PenNode],
                id: &str,
                path: &mut Vec<Value>,
                found: &mut Vec<Selection<'a>>,
            ) {
                for (index, node) in nodes.iter().enumerate() {
                    path.push(identity(node, index));
                    if node.base().id == id {
                        found.push(Selection {
                            node,
                            path: path.clone(),
                        });
                    }
                    if let Some(children) = node.children() {
                        visit(children, id, path, found);
                    }
                    path.pop();
                }
            }
            let mut found = Vec::new();
            visit(roots, id, &mut Vec::new(), &mut found);
            if found.len() != 1 {
                return Err(SelectorError::IdMatchCount {
                    id: id.clone(),
                    matches: found.len(),
                });
            }
            Ok(found.remove(0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tree() -> Vec<PenNode> {
        serde_json::from_value(json!([{"type":"frame","id":"root","name":"card","children":[{"type":"rectangle","id":"a","name":"tile"},{"type":"rectangle","id":"b","name":"tile"}]}])).unwrap()
    }
    #[test]
    fn descendant_paths_require_explicit_disambiguation() {
        let tree = tree();
        assert!(select(&tree, &Selector::path_json(r#"["card","tile"]"#).unwrap()).is_err());
        let selected = select(
            &tree,
            &Selector::path_json(r#"["card",{"name":"tile","index":1}]"#).unwrap(),
        )
        .unwrap();
        assert_eq!(selected.node.base().id, "b");
        assert_eq!(selected.path[1]["index"], 1);
        assert!(select(&tree, &Selector::top_level("missing".into())).is_err());
    }
    #[test]
    fn node_ids_are_unique_and_malformed_paths_are_rejected() {
        let mut tree = tree();
        assert_eq!(
            select(&tree, &Selector::Id("a".into()))
                .unwrap()
                .node
                .base()
                .name
                .as_deref(),
            Some("tile")
        );
        tree.push(tree[0].clone());
        assert!(select(&tree, &Selector::Id("a".into())).is_err());
        for path in [
            "[]",
            "{}",
            r#"[{"name":"tile","index":-1}]"#,
            r#"[{"name":"tile","occurrence":1}]"#,
        ] {
            assert!(Selector::path_json(path).is_err());
        }
    }
    #[test]
    fn nameless_roots_match_explicit_blank_segments_without_trimming_spaces() {
        let roots: Vec<PenNode> = serde_json::from_value(json!([
            {"type":"frame","id":"nameless","children":[{"type":"rectangle","id":"space","name":" "}]},
            {"type":"frame","id":"empty","name":""}
        ])).unwrap();
        let ambiguous = select(&roots, &Selector::top_level("".into()))
            .err()
            .unwrap();
        assert_eq!(ambiguous.detail()["matches"], 2);
        let path = Selector::path_json(r#"[{"name":"","index":0}," "]"#).unwrap();
        assert_eq!(select(&roots, &path).unwrap().node.base().id, "space");
    }
}
