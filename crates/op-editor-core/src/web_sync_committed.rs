//! Bounded committed state parsing; unknown operations explicitly require resync.

use jian_ops_schema::node::PenNode;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommittedFeedError {
    InvalidResponse,
    Precondition,
    Capture,
    Rollback,
}
impl std::fmt::Display for CommittedFeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidResponse => "committed changefeed response requires a fresh snapshot",
            Self::Precondition => "committed changefeed precondition changed; resync required",
            Self::Capture => "committed changefeed could not capture exact bounded undo state",
            Self::Rollback => {
                "committed changefeed repaint rollback no longer matches this document"
            }
        })
    }
}
impl std::error::Error for CommittedFeedError {}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Metadata {
    pub page: Option<Page>,
    pub path: Vec<usize>,
    pub expected_id: String,
    pub children_present: bool,
    pub value: PenNode,
}
#[derive(Deserialize)]
pub(crate) struct Page {
    pub index: usize,
    pub id: String,
}
#[derive(Deserialize)]
#[serde(tag = "op")]
pub(crate) enum Operation {
    #[serde(rename = "replace-node-metadata")]
    Metadata(Metadata),
    #[serde(rename = "replace-node")]
    Node(NodeValue),
    #[serde(rename = "replace-children")]
    Children(Children),
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NodeValue {
    pub page: Option<Page>,
    pub path: Vec<usize>,
    pub expected_id: String,
    pub value: PenNode,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Children {
    pub page: Option<Page>,
    pub parent_ids: Vec<String>,
    pub expected_ids: Vec<String>,
    pub expected_present: bool,
    pub value: Option<Vec<PenNode>>,
}
impl Operation {
    pub(crate) fn page(&self) -> Option<&Page> {
        match self {
            Self::Metadata(value) => value.page.as_ref(),
            Self::Node(value) => value.page.as_ref(),
            Self::Children(value) => value.page.as_ref(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    from_version: u64,
    to_version: u64,
    operations: Vec<Operation>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Wire {
    ok: bool,
    schema: u32,
    resync_required: bool,
    from_version: u64,
    version: u64,
    head_version: u64,
    cursor: String,
    more: bool,
    changes: Vec<Record>,
}

pub struct CommittedMetadata {
    pub version: u64,
    pub cursor: String,
    pub more: bool,
    pub changes_structure: bool,
    pub(crate) highest_id: u64,
    pub(crate) operations: Vec<Operation>,
}
impl CommittedMetadata {
    pub fn parse(body: &str, from_version: u64) -> Result<Self, CommittedFeedError> {
        if body.len() > 1024 * 1024 {
            return Err(CommittedFeedError::InvalidResponse);
        }
        let wire: Wire =
            serde_json::from_str(body).map_err(|_| CommittedFeedError::InvalidResponse)?;
        if !wire.ok
            || wire.schema != 1
            || wire.resync_required
            || wire.from_version != from_version
            || wire.changes.len() > 64
            || wire.version > wire.head_version
            || wire.more != (wire.version != wire.head_version)
            || !valid_cursor(&wire.cursor)
            || wire
                .cursor
                .rsplit('.')
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                != Some(wire.version)
        {
            return Err(CommittedFeedError::InvalidResponse);
        }
        let mut next = from_version;
        let mut operations = Vec::new();
        let mut changes_structure = false;
        let mut highest_id = 0;
        let mut nodes_left = 32768;
        for record in wire.changes {
            if record.from_version != next || record.to_version <= next {
                return Err(CommittedFeedError::InvalidResponse);
            }
            next = record.to_version;
            for operation in record.operations {
                if operations.len() == 4096 {
                    return Err(CommittedFeedError::InvalidResponse);
                }
                match &operation {
                    Operation::Metadata(value) => {
                        if value.path.is_empty() || value.path.len() > 128 {
                            return Err(CommittedFeedError::InvalidResponse);
                        }
                        changes_structure |=
                            value.expected_id != crate::PenNodeExt::id_str(&value.value);
                        inspect(
                            std::slice::from_ref(&value.value),
                            &mut nodes_left,
                            &mut highest_id,
                        )?;
                    }
                    Operation::Node(value) => {
                        if value.path.is_empty() || value.path.len() > 128 {
                            return Err(CommittedFeedError::InvalidResponse);
                        }
                        changes_structure = true;
                        inspect(
                            std::slice::from_ref(&value.value),
                            &mut nodes_left,
                            &mut highest_id,
                        )?;
                    }
                    Operation::Children(value) => {
                        if value.parent_ids.len() > 128 || value.expected_ids.len() > 4096 {
                            return Err(CommittedFeedError::InvalidResponse);
                        }
                        changes_structure = true;
                        inspect(
                            value.value.as_deref().unwrap_or(&[]),
                            &mut nodes_left,
                            &mut highest_id,
                        )?;
                    }
                }
                operations.push(operation);
            }
        }
        if next != wire.version {
            return Err(CommittedFeedError::InvalidResponse);
        }
        Ok(Self {
            version: wire.version,
            cursor: wire.cursor,
            more: wire.more,
            changes_structure,
            highest_id,
            operations,
        })
    }
}
fn inspect(
    nodes: &[PenNode],
    left: &mut usize,
    highest: &mut u64,
) -> Result<(), CommittedFeedError> {
    use crate::PenNodeExt;
    let mut stack = vec![(nodes.iter(), 0usize)];
    while let Some((nodes, depth)) = stack.last_mut() {
        let Some(node) = nodes.next() else {
            stack.pop();
            continue;
        };
        if *left == 0 || *depth > 128 {
            return Err(CommittedFeedError::InvalidResponse);
        }
        *left -= 1;
        *highest = (*highest).max(crate::walkers::parse_n_id(node.id_str()).unwrap_or(0));
        if let Some(children) = node.children() {
            let next = *depth + 1;
            stack.push((children.iter(), next));
        }
    }
    Ok(())
}
pub(super) fn valid_cursor(cursor: &str) -> bool {
    if cursor.len() > 128 {
        return false;
    }
    let mut parts = cursor.split('.');
    let Some(id) = parts.next() else {
        return false;
    };
    id.len() == 32
        && id.bytes().all(|byte| byte.is_ascii_hexdigit())
        && (0..3).all(|_| {
            parts
                .next()
                .is_some_and(|value| value.parse::<u64>().is_ok())
        })
        && parts.next().is_none()
}
