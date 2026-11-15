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
    #[serde(rename = "set-component-label")]
    CatalogLabel(CatalogLabel),
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
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogLabel {
    pub id: String,
    pub expected_name: String,
    pub name: String,
}
impl Operation {
    pub(crate) fn page(&self) -> Option<&Page> {
        match self {
            Self::Metadata(value) => value.page.as_ref(),
            Self::Node(value) => value.page.as_ref(),
            Self::Children(value) => value.page.as_ref(),
            Self::CatalogLabel(_) => None,
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
    from_version: u64,
    encoded_bytes: usize,
    incoming_nodes: usize,
    pub version: u64,
    pub cursor: String,
    pub more: bool,
    pub changes_structure: bool,
    pub(crate) highest_id: u64,
    pub(crate) operations: Vec<Operation>,
}
impl CommittedMetadata {
    /// Combine contiguous since pages before any live mutation. False means
    /// the bounded prefix is full; next is discarded and can be fetched again
    /// after that prefix is applied. Errors require resync, never partial data.
    pub fn append_page(&mut self, mut next: Self) -> Result<bool, CommittedFeedError> {
        if next.from_version != self.version
            || next.version < self.version
            || next.cursor.rsplit_once('.').map(|(authority, _)| authority)
                != self.cursor.rsplit_once('.').map(|(authority, _)| authority)
        {
            return Err(CommittedFeedError::InvalidResponse);
        }
        let bytes = self
            .encoded_bytes
            .checked_add(next.encoded_bytes)
            .ok_or(CommittedFeedError::InvalidResponse)?;
        let nodes = self
            .incoming_nodes
            .checked_add(next.incoming_nodes)
            .ok_or(CommittedFeedError::InvalidResponse)?;
        let operations = self
            .operations
            .len()
            .checked_add(next.operations.len())
            .ok_or(CommittedFeedError::InvalidResponse)?;
        if bytes > 4 * 1024 * 1024 || nodes > 32768 || operations > 4096 {
            return Ok(false);
        }
        if operations > self.operations.capacity() {
            let capacity = operations.next_power_of_two().min(4096);
            self.operations
                .reserve_exact(capacity - self.operations.len());
        }
        self.operations.append(&mut next.operations);
        self.encoded_bytes = bytes;
        self.incoming_nodes = nodes;
        self.version = next.version;
        self.cursor = next.cursor;
        self.more = next.more;
        self.changes_structure |= next.changes_structure;
        self.highest_id = self.highest_id.max(next.highest_id);
        Ok(true)
    }
    pub fn can_drain_more(&self) -> bool {
        self.more
            && self.encoded_bytes < 4 * 1024 * 1024
            && self.incoming_nodes < 32768
            && self.operations.len() < 4096
    }

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
                    Operation::CatalogLabel(value) => {
                        if value.id.is_empty()
                            || value.id.len() > 16 * 1024
                            || value.name.len().saturating_add(value.expected_name.len())
                                > 256 * 1024
                        {
                            return Err(CommittedFeedError::InvalidResponse);
                        }
                    }
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
            from_version,
            encoded_bytes: body.len(),
            incoming_nodes: 32768 - nodes_left,
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
