//! Bounded metadata feed parsing. Structural/new operation kinds deliberately
//! ask the web client to obtain a fresh snapshot until its applier supports them.

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
enum Operation {
    #[serde(rename = "replace-node-metadata")]
    Metadata(Metadata),
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
    pub(crate) operations: Vec<Metadata>,
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
        for record in wire.changes {
            if record.from_version != next || record.to_version <= next {
                return Err(CommittedFeedError::InvalidResponse);
            }
            next = record.to_version;
            for Operation::Metadata(operation) in record.operations {
                if operations.len() == 4096
                    || operation.path.is_empty()
                    || operation.path.len() > 128
                {
                    return Err(CommittedFeedError::InvalidResponse);
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
            operations,
        })
    }
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
