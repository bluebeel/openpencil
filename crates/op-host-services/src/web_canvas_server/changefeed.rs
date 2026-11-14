//! Bounded tenant-local committed STATE feed. Encoded records are shared across
//! readers; expiry and unsupported changes require an explicit fresh snapshot.

use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[path = "changefeed_http.rs"]
pub(super) mod http;

pub(super) const MAX_RECORD_BYTES: usize = 256 * 1024;
// Keep a1000-writer burst available while readers wait in the request queue.
const MAX_RETAINED_BYTES: usize = 64 * 1024 * 1024;
const MAX_RECORDS: usize = 4096;
pub(super) const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub(super) const MAX_RESPONSE_RECORDS: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Scope {
    epoch: u64,
    generation: u64,
}

/// Owned only while staging one exclusively locked request/cohort. Saturation
/// discards all partial data and becomes a resync, never a partial state diff.
#[derive(Default)]
pub(super) struct Staged {
    operations: Vec<u8>,
    reset: bool,
}
impl Staged {
    pub(super) fn capture(
        &mut self,
        capture: &op_editor_core::CommandEditCapture,
        editor: &op_editor_core::EditorState,
    ) {
        self.push(capture.forward_metadata_json(editor, MAX_RECORD_BYTES));
    }
    pub(super) fn unsupported(&mut self) {
        self.reset = true;
        self.operations = Vec::new();
    }
    pub(super) fn push(&mut self, encoded: Option<Vec<u8>>) {
        if self.reset {
            return;
        }
        let Some(encoded) =
            encoded.filter(|value| value.starts_with(b"[") && value.ends_with(b"]"))
        else {
            self.unsupported();
            return;
        };
        let body = &encoded[1..encoded.len() - 1];
        if body.is_empty() {
            return;
        }
        let comma = usize::from(!self.operations.is_empty());
        if self
            .operations
            .len()
            .saturating_add(body.len())
            .saturating_add(comma)
            > MAX_RECORD_BYTES - 256
        {
            self.unsupported();
            return;
        }
        let required = self.operations.len() + body.len() + comma;
        if required > self.operations.capacity() {
            let capacity = required.next_power_of_two().min(MAX_RECORD_BYTES);
            self.operations
                .reserve_exact(capacity - self.operations.len());
        }
        if comma != 0 {
            self.operations.push(b',');
        }
        self.operations.extend_from_slice(body);
    }
}

/// Charged until the LAST reader releases a record, even if it was evicted
/// from the ring. Slow response writers cannot hide evicted retained payloads.
#[derive(Default)]
struct LiveBytes(AtomicUsize);
pub(super) struct Record {
    from: u64,
    to: u64,
    encoded: Box<[u8]>,
    budget: Arc<LiveBytes>,
}
impl Drop for Record {
    fn drop(&mut self) {
        self.budget
            .0
            .fetch_sub(self.encoded.len(), Ordering::AcqRel);
    }
}
pub(super) struct Feed {
    id: String,
    scope: Option<Scope>,
    head: u64,
    floor: u64,
    barrier: &'static str,
    ring: VecDeque<Arc<Record>>,
    budget: Arc<LiveBytes>,
}
impl Default for Feed {
    fn default() -> Self {
        Self {
            id: super::random_token(),
            scope: None,
            head: 0,
            floor: 0,
            barrier: "snapshot-required",
            ring: VecDeque::new(),
            budget: Arc::default(),
        }
    }
}
impl Feed {
    pub(super) fn align(&mut self, scope: Scope, version: u64) {
        if self.scope != Some(scope) {
            self.reset(scope, version, "document-replaced");
        } else if self.head != version {
            // A REST/remote/direct mutation did not supply exact durable data.
            // No fabricated transaction fills this gap.
            self.reset(scope, version, "unsupported-change");
        }
    }
    fn reset(&mut self, scope: Scope, version: u64, reason: &'static str) {
        self.ring.clear();
        self.scope = Some(scope);
        self.head = version;
        self.floor = version;
        self.barrier = reason;
    }
    pub(super) fn cursor(&self) -> String {
        self.cursor_at(self.head)
    }
    fn cursor_at(&self, version: u64) -> String {
        let scope = self.scope.expect("aligned feed");
        format!(
            "{}.{}.{}.{}",
            self.id, scope.epoch, scope.generation, version
        )
    }
    pub(super) fn publish(&mut self, scope: Scope, from: u64, to: u64, staged: Staged) {
        self.align(scope, from);
        if to <= from {
            return;
        }
        if staged.reset {
            self.reset(scope, to, "unsupported-change");
            return;
        }
        let prefix = format!("{{\"fromVersion\":{from},\"toVersion\":{to},\"operations\":[");
        let mut encoded = Vec::with_capacity(prefix.len() + staged.operations.len() + 2);
        encoded.extend_from_slice(prefix.as_bytes());
        encoded.extend_from_slice(&staged.operations);
        encoded.extend_from_slice(b"]}");
        if encoded.len() > MAX_RECORD_BYTES {
            self.reset(scope, to, "delta-too-large");
            return;
        }
        while self.ring.len() >= MAX_RECORDS {
            self.evict();
        }
        let len = encoded.len();
        loop {
            let reserved = self
                .budget
                .0
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |bytes| {
                    bytes
                        .checked_add(len)
                        .filter(|next| *next <= MAX_RETAINED_BYTES)
                })
                .is_ok();
            if reserved {
                break;
            }
            if self.ring.is_empty() {
                self.reset(scope, to, "retained-response-capacity");
                return;
            }
            self.evict();
        }
        self.ring.push_back(Arc::new(Record {
            from,
            to,
            encoded: encoded.into_boxed_slice(),
            budget: Arc::clone(&self.budget),
        }));
        self.head = to;
    }
    fn evict(&mut self) {
        if let Some(record) = self.ring.pop_front() {
            self.floor = record.to;
            self.barrier = "cursor-expired";
        }
    }
    pub(super) fn since(&self, cursor: &str, limit: usize) -> Batch {
        let parsed = self.parse(cursor);
        let reason = match parsed {
            None => Some("cursor-authority-changed"),
            Some(version) if version > self.head => Some("cursor-ahead"),
            Some(version) if version < self.floor => Some(self.barrier),
            _ => None,
        };
        if let Some(reason) = reason {
            return Batch::Reset {
                cursor: self.cursor(),
                version: self.head,
                reason,
            };
        }
        let from = parsed.unwrap();
        let mut next = from;
        let mut bytes = 0;
        let mut records = Vec::new();
        for record in self.ring.iter().filter(|record| record.to > from) {
            if record.from != next {
                return Batch::Reset {
                    cursor: self.cursor(),
                    version: self.head,
                    reason: "cursor-gap",
                };
            }
            if records.len() >= limit || bytes + record.encoded.len() + 4096 > MAX_RESPONSE_BYTES {
                break;
            }
            bytes += record.encoded.len() + 1;
            next = record.to;
            records.push(Arc::clone(record));
        }
        if from != self.head && records.is_empty() {
            return Batch::Reset {
                cursor: self.cursor(),
                version: self.head,
                reason: "cursor-gap",
            };
        }
        Batch::Data {
            from,
            next: self.cursor_at(next),
            version: next,
            head: self.head,
            more: next != self.head,
            records,
        }
    }
    fn parse(&self, cursor: &str) -> Option<u64> {
        if cursor.len() > 128 {
            return None;
        }
        let mut parts = cursor.split('.');
        if parts.next()? != self.id {
            return None;
        }
        let scope = self.scope?;
        if parts.next()?.parse::<u64>().ok()? != scope.epoch
            || parts.next()?.parse::<u64>().ok()? != scope.generation
        {
            return None;
        }
        let version = parts.next()?.parse().ok()?;
        parts.next().is_none().then_some(version)
    }
    pub(super) fn stats(&self) -> serde_json::Value {
        serde_json::json!({"schema":1,"endpoint":"/api/mcp/changes","records":self.ring.len(),"retainedEncodedBytes":self.budget.0.load(Ordering::Acquire),"maxRetainedEncodedBytes":MAX_RETAINED_BYTES,"maxRetainedRecords":MAX_RECORDS,"maxRecordBytes":MAX_RECORD_BYTES,"maxResponseBytes":MAX_RESPONSE_BYTES,"data":"exact node metadata and bounded structural state; broad or oversized changes require resync","durability":"Managed bound command save-before-ack only"})
    }
}

pub(super) enum Batch {
    Reset {
        cursor: String,
        version: u64,
        reason: &'static str,
    },
    Data {
        from: u64,
        next: String,
        version: u64,
        head: u64,
        more: bool,
        records: Vec<Arc<Record>>,
    },
}

impl super::WebCanvasState {
    fn feed_scope(&self) -> Scope {
        Scope {
            epoch: self.recovery.epoch(),
            generation: self.editor.document_generation(),
        }
    }
    pub(super) fn change_cursor(&mut self) -> String {
        let scope = self.feed_scope();
        self.changefeed.align(scope, self.version);
        self.changefeed.cursor()
    }
    /// Call only after confirmed durable save while holding the state mutex.
    pub(super) fn publish_committed_changes(&mut self, from: u64, to: u64, staged: Staged) {
        if self.recovery.required() {
            return;
        }
        let scope = self.feed_scope();
        self.changefeed.publish(scope, from, to, staged);
    }
}
