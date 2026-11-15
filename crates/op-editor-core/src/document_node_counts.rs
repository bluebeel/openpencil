//! Immutable physical-canonical counts behind the existing explicit host read
//! authority. No ID/location index, resource expansion or node clone is built.
use crate::{EditorState, PenNodeExt};
use jian_ops_schema::node::PenNode;
use std::{
    collections::VecDeque,
    mem, slice,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, LazyLock, Mutex, Weak,
    },
};
const NAMES: [&str; 21] = [
    "checkbox",
    "ellipse",
    "frame",
    "group",
    "icon_font",
    "image",
    "line",
    "number_input",
    "path",
    "polygon",
    "progress",
    "radio_group",
    "rectangle",
    "ref",
    "select",
    "slider",
    "switch",
    "tabs",
    "text",
    "text_area",
    "text_input",
];
const MAX_FILES: usize = 64;
// Reserve fixed cache-directory/allocator bookkeeping beside charged values.
const MAX_BYTES: usize = 16 * 1024 * 1024 - 64 * 1024;
const MAX_ENTRY: usize = 4 * 1024 * 1024;
const MAX_DEPTH: usize = 65536;
const SCRATCH: usize = MAX_DEPTH * mem::size_of::<slice::Iter<'static, PenNode>>() + 4096;
static BYTES: AtomicUsize = AtomicUsize::new(0);
struct Reservation(usize);
impl Reservation {
    fn acquire(bytes: usize) -> Option<Self> {
        BYTES
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|total| *total <= MAX_BYTES)
            })
            .ok()
            .map(|_| Self(bytes))
    }
    fn shrink(&mut self, bytes: usize) -> Option<()> {
        let released = self.0.checked_sub(bytes)?;
        BYTES.fetch_sub(released, Ordering::AcqRel);
        self.0 = bytes;
        Some(())
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        BYTES.fetch_sub(self.0, Ordering::AcqRel);
    }
}
/// Counts every physical authored node, including duplicates, Ref authored
/// children and hidden nodes exactly as the ordinary complete canonical walk.
#[derive(Default)]
pub struct ForestNodeCounts {
    total: usize,
    kinds: [usize; 21],
}
impl ForestNodeCounts {
    pub fn total(&self) -> usize {
        self.total
    }
    /// Canonical labels in the same lexical order as a BTreeMap histogram.
    pub fn histogram(&self) -> impl Iterator<Item = (&'static str, usize)> + '_ {
        NAMES
            .iter()
            .zip(self.kinds.iter())
            .filter_map(|(name, count)| (*count != 0).then_some((*name, *count)))
    }
}
/// The Arc owns its byte reservation. Evicting an entry cannot uncharge an
/// old result still pinned by a caller, nor can an allocator address be reused
/// while the weak document identity is retained.
pub struct DocumentNodeCounts {
    root: ForestNodeCounts,
    pages: Option<Vec<ForestNodeCounts>>,
    canonical_total: usize,
    _reservation: Reservation,
}
impl DocumentNodeCounts {
    pub fn root(&self) -> &ForestNodeCounts {
        &self.root
    }
    pub fn pages(&self) -> Option<&[ForestNodeCounts]> {
        self.pages.as_deref()
    }
    /// Some(pages), INCLUDING Some([]), sums pages; None counts root children.
    pub fn canonical_total(&self) -> usize {
        self.canonical_total
    }
    /// Matches EditorState::active_children: empty/absent pages use the root.
    pub fn active_page(&self, index: usize) -> &ForestNodeCounts {
        match self.pages.as_ref().filter(|pages| !pages.is_empty()) {
            Some(pages) => &pages[index.min(pages.len() - 1)],
            None => &self.root,
        }
    }
}
fn kind(node: &PenNode) -> usize {
    match node {
        PenNode::Checkbox(_) => 0,
        PenNode::Ellipse(_) => 1,
        PenNode::Frame(_) => 2,
        PenNode::Group(_) => 3,
        PenNode::IconFont(_) => 4,
        PenNode::Image(_) => 5,
        PenNode::Line(_) => 6,
        PenNode::NumberInput(_) => 7,
        PenNode::Path(_) => 8,
        PenNode::Polygon(_) => 9,
        PenNode::Progress(_) => 10,
        PenNode::RadioGroup(_) => 11,
        PenNode::Rectangle(_) => 12,
        PenNode::Ref(_) => 13,
        PenNode::Select(_) => 14,
        PenNode::Slider(_) => 15,
        PenNode::Switch(_) => 16,
        PenNode::Tabs(_) => 17,
        PenNode::Text(_) => 18,
        PenNode::TextArea(_) => 19,
        PenNode::TextInput(_) => 20,
    }
}
fn forest<'a>(
    roots: &'a [PenNode],
    pending: &mut Vec<slice::Iter<'a, PenNode>>,
) -> Option<ForestNodeCounts> {
    let mut counts = ForestNodeCounts::default();
    pending.clear();
    pending.push(roots.iter());
    while let Some(siblings) = pending.last_mut() {
        let Some(node) = siblings.next() else {
            pending.pop();
            continue;
        };
        counts.total = counts.total.checked_add(1)?;
        let count = &mut counts.kinds[kind(node)];
        *count = count.checked_add(1)?;
        if let Some(children) = node.children().filter(|children| !children.is_empty()) {
            if pending.len() == MAX_DEPTH {
                return None;
            }
            pending.push(children.iter());
        }
    }
    Some(counts)
}
struct Value {
    generation: u64,
    revision: u64,
    counts: Arc<DocumentNodeCounts>,
}
struct Slot {
    owner: Weak<()>,
    value: Mutex<Option<Value>>,
    _reservation: Reservation,
}
#[derive(Default)]
struct Cache {
    slots: VecDeque<Arc<Slot>>,
}
impl Cache {
    fn evict_one(&mut self) -> bool {
        let Some(index) = self
            .slots
            .iter()
            .position(|slot| Arc::strong_count(slot) == 1)
        else {
            return false;
        };
        self.slots.remove(index);
        true
    }
    fn slot(&mut self, state: &EditorState) -> Option<Arc<Slot>> {
        self.slots
            .retain(|slot| slot.owner.strong_count() != 0 || Arc::strong_count(slot) > 1);
        let owner = state.derived_cache_owner();
        if let Some(index) = self.slots.iter().position(|slot| slot.owner.ptr_eq(&owner)) {
            let slot = self.slots.remove(index)?;
            self.slots.push_back(Arc::clone(&slot));
            return Some(slot);
        }
        while self.slots.len() >= MAX_FILES {
            if !self.evict_one() {
                return None;
            }
        }
        let charge = mem::size_of::<Slot>() + 64;
        let reservation = loop {
            if let Some(reservation) = Reservation::acquire(charge) {
                break reservation;
            }
            if !self.evict_one() {
                return None;
            }
        };
        self.slots.try_reserve(1).ok()?;
        let slot = Arc::new(Slot {
            owner,
            value: Mutex::new(None),
            _reservation: reservation,
        });
        self.slots.push_back(Arc::clone(&slot));
        Some(slot)
    }
}
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(Cache::default()));
fn reserve(bytes: usize) -> Option<Reservation> {
    if let Some(value) = Reservation::acquire(bytes) {
        return Some(value);
    }
    let mut cache = CACHE.lock().ok()?;
    loop {
        if !cache.evict_one() {
            return None;
        }
        if let Some(value) = Reservation::acquire(bytes) {
            return Some(value);
        }
    }
}
fn build(state: &EditorState) -> Option<Arc<DocumentNodeCounts>> {
    let page_count = state.doc.pages.as_ref().map_or(0, Vec::len);
    let predicted = page_count
        .checked_mul(mem::size_of::<ForestNodeCounts>())?
        .checked_mul(2)?
        .checked_add(mem::size_of::<DocumentNodeCounts>() + 4096)?;
    if predicted > MAX_ENTRY {
        return None;
    }
    let mut reservation = reserve(predicted.checked_add(SCRATCH)?)?;
    let mut pending = Vec::new();
    let root = forest(&state.doc.children, &mut pending)?;
    let pages = match &state.doc.pages {
        None => None,
        Some(pages) => {
            let mut out = Vec::new();
            out.try_reserve_exact(pages.len()).ok()?;
            for page in pages {
                out.push(forest(&page.children, &mut pending)?);
            }
            Some(out)
        }
    };
    drop(pending);
    let canonical_total = match &pages {
        Some(pages) => pages
            .iter()
            .try_fold(0usize, |total, page| total.checked_add(page.total))?,
        None => root.total,
    };
    let actual = mem::size_of::<DocumentNodeCounts>()
        + 64
        + pages.as_ref().map_or(0, |pages| {
            pages.capacity() * mem::size_of::<ForestNodeCounts>()
        });
    if actual > MAX_ENTRY {
        return None;
    }
    reservation.shrink(actual)?;
    Some(Arc::new(DocumentNodeCounts {
        root,
        pages,
        canonical_total,
        _reservation: reservation,
    }))
}
impl EditorState {
    /// Only between completed commands inside an existing explicit locked
    /// trusted read/write authority. Outside it return None for the caller's
    /// fresh full-document factory. No lookup/index construction is requested.
    pub fn trusted_document_node_counts(&self) -> Option<Arc<DocumentNodeCounts>> {
        let seed = &self.command_id_seed;
        let generation = self.document_generation();
        let revision = self.document_revision();
        if !seed.entered
            || !seed.valid
            || seed.generation != generation
            || generation == u64::MAX
            || revision == u64::MAX
        {
            return None;
        }
        let slot = CACHE.lock().ok()?.slot(self)?;
        // File-local coalescing: neither the forest walk nor a waiting caller
        // holds the global directory mutex or blocks an unrelated file build.
        let mut value = slot.value.lock().ok()?;
        if let Some(value) = value
            .as_ref()
            .filter(|value| value.generation == generation && value.revision == revision)
        {
            return Some(Arc::clone(&value.counts));
        }
        // The previous value's reservation stays alive if another caller
        // still pins its Arc; removing the cache handle never hides that RAM.
        *value = None;
        let counts = build(self)?;
        if self.document_generation() != generation || self.document_revision() != revision {
            return None;
        }
        *value = Some(Value {
            generation,
            revision,
            counts: Arc::clone(&counts),
        });
        Some(counts)
    }
}
