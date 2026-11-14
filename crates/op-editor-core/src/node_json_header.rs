//! Root-only JSON updates preserve descendant ownership and exact metadata.
use crate::{pen_node_ext::clone_without_children, PenNodeExt};
use jian_ops_schema::node::PenNode;
use serde_json::Value;

#[derive(Debug)]
pub(crate) enum HeaderMapError {
    Json(serde_json::Error),
    TypeChanged,
    ChildPresenceChanged,
    MissingId,
}
impl From<serde_json::Error> for HeaderMapError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}
impl std::fmt::Display for HeaderMapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => std::fmt::Display::fmt(error, f),
            Self::TypeChanged => f.write_str("header mapping changed node type"),
            Self::ChildPresenceChanged => f.write_str("header mapping changed child presence"),
            Self::MissingId => f.write_str("header mapping removed node id"),
        }
    }
}
impl std::error::Error for HeaderMapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        if let Self::Json(error) = self {
            Some(error)
        } else {
            None
        }
    }
}
pub(crate) fn map_header_json_in_place(
    node: &mut PenNode,
    transform: impl FnOnce(&mut Value),
) -> Result<(), HeaderMapError> {
    let mut value = if node.children().is_none_or(|children| children.is_empty()) {
        serde_json::to_value(&*node)?
    } else {
        serde_json::to_value(clone_without_children(node))?
    };
    transform(&mut value);
    let header: PenNode = serde_json::from_value(value)?;
    if std::mem::discriminant(node) != std::mem::discriminant(&header) {
        return Err(HeaderMapError::TypeChanged);
    }
    if node.children().is_some() != header.children().is_some() {
        return Err(HeaderMapError::ChildPresenceChanged);
    }
    if header.id_str().is_empty() {
        return Err(HeaderMapError::MissingId);
    }
    // All parsing/validation completes before touching the canonical node.
    // The old child Vec is moved into its new header, never recursively cloned
    // or decoded through JSON. Authored absent/present-empty states stay exact.
    let mut old = std::mem::replace(node, header);
    if old.children().is_some() {
        let children = std::mem::take(old.children_mut().unwrap());
        *node.children_mut().unwrap() = children;
    }
    Ok(())
}
