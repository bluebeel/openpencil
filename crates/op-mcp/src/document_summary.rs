//! Small shared summaries for revision-authoritative host dispatch. Public
//! arbitrary-document snapshot constructors remain fresh and uncached.

use crate::{
    read_tools_extra::{CountNodes, ListNodeKinds},
    tools::GetDocumentInfo,
};
use jian_ops_schema::node::PenNode;
use op_editor_core::{EditorState, PenNodeExt};
use std::sync::{Arc, Mutex, OnceLock, Weak};

const MAX_ENTRIES: usize = 128;
const MAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Counts,
    Kinds(usize),
}
struct Key {
    owner: Weak<()>,
    generation: u64,
    revision: u64,
    kind: Kind,
}
impl Key {
    fn new(state: &EditorState, kind: Kind) -> Self {
        Self {
            owner: state.derived_cache_owner(),
            generation: state.document_generation(),
            revision: state.document_revision(),
            kind,
        }
    }
    fn matches(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.owner, &other.owner)
            && self.generation == other.generation
            && self.revision == other.revision
            && self.kind == other.kind
    }
}
#[derive(Clone)]
enum Summary {
    Counts(Arc<Vec<usize>>),
    Kinds(Arc<Vec<(String, u32)>>),
}
impl Summary {
    fn bytes(&self) -> usize {
        std::mem::size_of::<Entry>().saturating_add(match self {
            Self::Counts(values) => values
                .capacity()
                .saturating_mul(std::mem::size_of::<usize>()),
            Self::Kinds(values) => values
                .capacity()
                .saturating_mul(std::mem::size_of::<(String, u32)>())
                .saturating_add(
                    values
                        .iter()
                        .map(|(name, _)| name.capacity())
                        .sum::<usize>(),
                ),
        })
    }
}
struct Entry {
    key: Key,
    value: Summary,
    bytes: usize,
}

fn get_or_build(key: Key, build: impl FnOnce() -> Summary) -> Summary {
    static CACHE: OnceLock<Mutex<Vec<Entry>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(Vec::new()));
    {
        let mut entries = cache.lock().unwrap_or_else(|poison| poison.into_inner());
        entries.retain(|entry| entry.key.owner.strong_count() != 0);
        if let Some(entry) = entries.iter().find(|entry| entry.key.matches(&key)) {
            return entry.value.clone();
        }
    }
    // Different files build independently; no tree walk under a global mutex.
    // One document's host authority already serializes its own cold readers.
    let value = build();
    let bytes = value.bytes();
    if bytes > MAX_BYTES {
        return value;
    }
    let mut entries = cache.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(entry) = entries.iter().find(|entry| entry.key.matches(&key)) {
        return entry.value.clone();
    }
    entries.retain(|entry| {
        entry.key.owner.strong_count() != 0
            && (!Weak::ptr_eq(&entry.key.owner, &key.owner)
                || (entry.key.generation == key.generation && entry.key.revision == key.revision))
    });
    let mut retained = entries.iter().map(|entry| entry.bytes).sum::<usize>();
    while entries.len() >= MAX_ENTRIES || retained.saturating_add(bytes) > MAX_BYTES {
        let old = entries.remove(0);
        retained = retained.saturating_sub(old.bytes);
    }
    entries.push(Entry {
        key,
        value: value.clone(),
        bytes,
    });
    value
}

fn count(roots: &[PenNode]) -> usize {
    let mut total = 0;
    let mut stack = vec![roots.iter()];
    while let Some(siblings) = stack.last_mut() {
        let Some(node) = siblings.next() else {
            stack.pop();
            continue;
        };
        total += 1;
        if let Some(children) = node.children().filter(|children| !children.is_empty()) {
            stack.push(children.iter());
        }
    }
    total
}
fn counts(state: &EditorState) -> Arc<Vec<usize>> {
    let value = get_or_build(Key::new(state, Kind::Counts), || {
        Summary::Counts(Arc::new(match &state.doc.pages {
            Some(pages) => pages.iter().map(|page| count(&page.children)).collect(),
            None => vec![count(&state.doc.children)],
        }))
    });
    match value {
        Summary::Counts(values) => values,
        Summary::Kinds(_) => unreachable!("summary key kind"),
    }
}

/// Requires exclusive host revision/generation authority, including all reloads.
pub fn document_info_snapshot_cached(state: &EditorState) -> GetDocumentInfo {
    GetDocumentInfo {
        page_count: state.page_count(),
        active_page_index: state.ui.active_page_index,
        total_nodes: counts(state).iter().sum(),
    }
}
/// Same authority contract as document_info_snapshot_cached. No histogram is
/// built for count-only callers, so cold scalar summaries retain linear cost.
pub fn count_nodes_snapshot_cached(state: &EditorState) -> CountNodes {
    CountNodes {
        per_page: counts(state)
            .iter()
            .map(|count| {
                debug_assert!(u32::try_from(*count).is_ok(), "node count overflow");
                *count as u32
            })
            .collect(),
    }
}
/// Cache the active canonical page only; caller-specific page selection is
/// observed afresh, while counts and histograms share no mutable tool state.
pub fn list_node_kinds_snapshot_cached(state: &EditorState) -> ListNodeKinds {
    let page = state.ui.active_page_index.min(
        state
            .doc
            .pages
            .as_ref()
            .map_or(0, |pages| pages.len().saturating_sub(1)),
    );
    let value = get_or_build(Key::new(state, Kind::Kinds(page)), || {
        Summary::Kinds(Arc::new(
            crate::read_tools_extra::list_node_kinds_snapshot(state).histogram,
        ))
    });
    match value {
        Summary::Kinds(histogram) => ListNodeKinds {
            histogram: (*histogram).clone(),
        },
        Summary::Counts(_) => unreachable!("summary key kind"),
    }
}
