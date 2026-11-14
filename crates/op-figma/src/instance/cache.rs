//! Bounded memoization for fully-resolved component instance children.
//!
//! Figma libraries often repeat the exact same component payload thousands of
//! times. Resolution is deterministic for a component, its override/derived
//! payload, instance size, and the current virtual-GUID assignment epoch. The
//! cache keeps that exact structural key and always deep-clones a hit so the
//! conversion pass can assign independent Pen ids and mutate its local tree.

use super::apply_instance_overrides_cached;
use crate::figma_types::FigVec2;
use crate::kiwi::FigValue;
use crate::tree::TreeNode;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

const DEFAULT_MAX_ENTRIES: usize = 256;
const DEFAULT_MAX_STRUCTURAL_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ExactSize {
    x_bits: u64,
    y_bits: u64,
}

impl From<FigVec2> for ExactSize {
    fn from(size: FigVec2) -> Self {
        Self {
            x_bits: size.x.to_bits(),
            y_bits: size.y.to_bits(),
        }
    }
}

#[derive(Clone)]
struct InstanceExpansionKey {
    component_guid: String,
    overrides: Vec<FigValue>,
    derived: Vec<FigValue>,
    size: Option<ExactSize>,
    global_assignment_epoch: u64,
    assignment_epoch: u64,
}

impl InstanceExpansionKey {
    fn new(
        component_guid: &str,
        overrides: Vec<FigValue>,
        derived: Vec<FigValue>,
        size: Option<FigVec2>,
        global_assignment_epoch: u64,
        assignment_epoch: u64,
    ) -> Self {
        Self {
            component_guid: component_guid.to_string(),
            overrides,
            derived,
            size: size.map(ExactSize::from),
            global_assignment_epoch,
            assignment_epoch,
        }
    }

    fn structural_bytes(&self) -> usize {
        // The key is Rc-shared by the map and FIFO queue. Count its actual
        // backing vectors, including capacity not occupied by live values.
        std::mem::size_of::<Self>()
            + 2 * std::mem::size_of::<usize>()
            + self.component_guid.capacity()
            + (self.overrides.capacity() - self.overrides.len()) * std::mem::size_of::<FigValue>()
            + self.overrides.iter().map(fig_value_bytes).sum::<usize>()
            + (self.derived.capacity() - self.derived.len()) * std::mem::size_of::<FigValue>()
            + self.derived.iter().map(fig_value_bytes).sum::<usize>()
    }
}

impl PartialEq for InstanceExpansionKey {
    fn eq(&self, other: &Self) -> bool {
        self.component_guid == other.component_guid
            && self.global_assignment_epoch == other.global_assignment_epoch
            && self.assignment_epoch == other.assignment_epoch
            && self.size == other.size
            && fig_values_equal(&self.overrides, &other.overrides)
            && fig_values_equal(&self.derived, &other.derived)
    }
}

impl Eq for InstanceExpansionKey {}

impl Hash for InstanceExpansionKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.component_guid.hash(state);
        self.global_assignment_epoch.hash(state);
        self.assignment_epoch.hash(state);
        self.size.hash(state);
        hash_fig_values(&self.overrides, state);
        hash_fig_values(&self.derived, state);
    }
}

fn fig_values_equal(left: &[FigValue], right: &[FigValue]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| fig_value_equal(left, right))
}

fn fig_value_equal(left: &FigValue, right: &FigValue) -> bool {
    let (left, right) = (left.resolved(), right.resolved());
    if let (Some(left), Some(right)) = (left.guid_parts(), right.guid_parts()) {
        return left == right;
    }
    if let (Some(left), Some(right)) = (left.affine_parts(), right.affine_parts()) {
        return left
            .iter()
            .zip(right)
            .all(|(left, right)| left.to_bits() == right.to_bits());
    }
    if let (Some((lk, lv)), Some((rk, rv))) = (left.small_float_parts(), right.small_float_parts())
    {
        return lk == rk && lv.iter().zip(rv).all(|(a, b)| a.to_bits() == b.to_bits());
    }
    if let (Some(left), Some(right)) = (left.as_str(), right.as_str()) {
        return left == right;
    }
    match (left, right) {
        (FigValue::Null, FigValue::Null) => true,
        (FigValue::Bool(a), FigValue::Bool(b)) => a == b,
        (FigValue::Int(a), FigValue::Int(b)) => a == b,
        (FigValue::Uint(a), FigValue::Uint(b)) => a == b,
        (FigValue::Int64(a), FigValue::Int64(b)) => a == b,
        (FigValue::Uint64(a), FigValue::Uint64(b)) => a == b,
        (FigValue::Float(a), FigValue::Float(b)) => a.to_bits() == b.to_bits(),
        (FigValue::Str(a), FigValue::Str(b)) => a == b,
        (FigValue::Bytes(a), FigValue::Bytes(b)) => a == b,
        (FigValue::Array(a), FigValue::Array(b)) => fig_values_equal(a, b),
        (FigValue::Object(a), FigValue::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|((ak, av), (bk, bv))| ak == bk && fig_value_equal(av, bv))
        }
        (FigValue::TextEvidence(a), FigValue::TextEvidence(b)) => a == b,
        (FigValue::TextLayout(a), FigValue::TextLayout(b)) => a == b,
        _ => false,
    }
}

fn hash_fig_values<H: Hasher>(values: &[FigValue], state: &mut H) {
    values.len().hash(state);
    for value in values.iter() {
        hash_fig_value(value, state);
    }
}

fn hash_fig_value<H: Hasher>(value: &FigValue, state: &mut H) {
    let value = value.resolved();
    if let Some(values) = value.affine_parts() {
        "Affine".hash(state);
        for value in values.iter() {
            value.to_bits().hash(state);
        }
        return;
    }
    if let Some(guid) = value.guid_parts() {
        "GUID".hash(state);
        guid.hash(state);
        return;
    }
    if let Some((kind, values)) = value.small_float_parts() {
        kind.hash(state);
        for value in values.iter() {
            value.to_bits().hash(state);
        }
        return;
    }
    if let Some(text) = value.as_str() {
        "string".hash(state);
        text.hash(state);
        return;
    }
    std::mem::discriminant(value).hash(state);
    match value {
        FigValue::Vec2(_) | FigValue::Color(_) => unreachable!("normalized before hashing"),
        FigValue::Symbol(_) => unreachable!(),
        FigValue::Deferred(_) => unreachable!(),
        FigValue::Null => {}
        FigValue::Bool(value) => value.hash(state),
        FigValue::Int(value) => value.hash(state),
        FigValue::Uint(value) => value.hash(state),
        FigValue::Int64(value) => value.hash(state),
        FigValue::Uint64(value) => value.hash(state),
        FigValue::Guid(session, local) => (session, local).hash(state),
        FigValue::Affine(values) => {
            for value in values.iter() {
                value.to_bits().hash(state);
            }
        }
        FigValue::Float(value) => value.to_bits().hash(state),
        FigValue::Str(value) => value.hash(state),
        FigValue::Bytes(value) => value.hash(state),
        FigValue::TextEvidence(value) => value.hash(state),
        FigValue::TextLayout(value) => value.hash(state),
        FigValue::Array(values) => hash_fig_values(values, state),
        FigValue::Object(pairs) => {
            pairs.len().hash(state);
            for (key, value) in pairs {
                key.hash(state);
                hash_fig_value(value, state);
            }
        }
    }
}

pub(crate) fn fig_value_bytes(value: &FigValue) -> usize {
    std::mem::size_of::<FigValue>() + fig_value_owned_bytes(value)
}

/// Include spare Vec/String capacities. Charging the full shared key payload
/// for every occurrence deliberately overcounts sharing and stays conservative.
fn fig_value_owned_bytes(value: &FigValue) -> usize {
    match value {
        FigValue::Deferred(value) => value.retained_bound(&fig_value_owned_bytes),
        FigValue::Vec2(value) => value.retained_bytes(),
        FigValue::Color(value) => {
            crate::kiwi::storage::boxed_color_bytes() + value.retained_bytes()
        }
        FigValue::Affine(_) => crate::kiwi::storage::boxed_affine_bytes(),
        FigValue::Symbol(value) => value.retained_bytes(),
        FigValue::Str(value) => crate::kiwi::storage::text_capacity(value),
        FigValue::Bytes(value) => crate::kiwi::storage::bytes_capacity(value),
        FigValue::TextEvidence(value) => value.retained_bytes(),
        FigValue::TextLayout(value) => value.retained_bytes(),
        FigValue::Array(values) => {
            values.capacity() * std::mem::size_of::<FigValue>()
                + values.iter().map(fig_value_owned_bytes).sum::<usize>()
        }
        FigValue::Object(pairs) => {
            pairs.capacity() * std::mem::size_of::<(crate::kiwi::FigKey, FigValue)>()
                + pairs
                    .iter()
                    .map(|(key, value)| key.retained_bytes() + fig_value_owned_bytes(value))
                    .sum::<usize>()
        }
        _ => 0,
    }
}

fn tree_structural_bytes(node: &TreeNode) -> usize {
    std::mem::size_of::<TreeNode>()
        + fig_value_owned_bytes(&node.figma)
        + (node.children.capacity() - node.children.len()) * std::mem::size_of::<TreeNode>()
        + node
            .children
            .iter()
            .map(tree_structural_bytes)
            .sum::<usize>()
}

struct CachedExpansion {
    children: Vec<TreeNode>,
    structural_bytes: usize,
}

/// Per-import, FIFO-bounded cache. Keys in `insertion_order` are `Rc`-shared
/// with the map, so large override payloads are not duplicated for eviction.
pub(crate) struct InstanceExpansionCache {
    entries: HashMap<Rc<InstanceExpansionKey>, CachedExpansion>,
    insertion_order: VecDeque<Rc<InstanceExpansionKey>>,
    assignment_epochs: HashMap<String, u64>,
    global_assignment_epoch: u64,
    structural_bytes: usize,
    max_entries: usize,
    max_structural_bytes: usize,
    hits: u64,
    misses: u64,
}

impl Default for InstanceExpansionCache {
    fn default() -> Self {
        Self::with_limits(DEFAULT_MAX_ENTRIES, DEFAULT_MAX_STRUCTURAL_BYTES)
    }
}

impl InstanceExpansionCache {
    fn with_limits(max_entries: usize, max_structural_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            insertion_order: VecDeque::new(),
            assignment_epochs: HashMap::new(),
            global_assignment_epoch: 0,
            structural_bytes: 0,
            max_entries,
            max_structural_bytes,
            hits: 0,
            misses: 0,
        }
    }

    fn assignment_epoch(&self, component_guid: &str) -> u64 {
        self.assignment_epochs
            .get(component_guid)
            .copied()
            .unwrap_or(0)
    }

    fn bump_assignment_epoch(&mut self, component_guid: &str) -> u64 {
        let epoch = self
            .assignment_epochs
            .entry(component_guid.to_string())
            .or_default();
        *epoch = epoch
            .checked_add(1)
            .expect("instance assignment epoch overflow");
        self.global_assignment_epoch = self
            .global_assignment_epoch
            .checked_add(1)
            .expect("global instance assignment epoch overflow");

        // Outer expansions can contain nested instances whose result depends on
        // this pin. A global fence conservatively invalidates those dependencies.
        self.entries.clear();
        self.insertion_order.clear();
        self.structural_bytes = 0;
        *epoch
    }

    fn get(&mut self, key: &InstanceExpansionKey) -> Option<Vec<TreeNode>> {
        if let Some(entry) = self.entries.get(key) {
            self.hits += 1;
            return Some(entry.children.clone());
        }
        self.misses += 1;
        None
    }

    fn insert(&mut self, key: InstanceExpansionKey, children: &[TreeNode]) {
        if self.max_entries == 0 || self.max_structural_bytes == 0 {
            return;
        }
        let structural_bytes =
            key.structural_bytes() + children.iter().map(tree_structural_bytes).sum::<usize>();
        if structural_bytes > self.max_structural_bytes || self.entries.contains_key(&key) {
            return;
        }
        while !self.entries.is_empty()
            && (self.entries.len() >= self.max_entries
                || self.structural_bytes.saturating_add(structural_bytes)
                    > self.max_structural_bytes)
        {
            let Some(oldest) = self.insertion_order.pop_front() else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.structural_bytes =
                    self.structural_bytes.saturating_sub(entry.structural_bytes);
            }
        }
        let key = Rc::new(key);
        self.insertion_order.push_back(Rc::clone(&key));
        self.structural_bytes = self.structural_bytes.saturating_add(structural_bytes);
        self.entries.insert(
            key,
            CachedExpansion {
                children: children.to_vec(),
                structural_bytes,
            },
        );
    }
}

/// Resolve one instance, reusing an exact prior expansion when safe.
pub(crate) fn apply_instance_overrides_memoized(
    cache: &mut InstanceExpansionCache,
    component_guid: &str,
    symbol_node: &TreeNode,
    overrides: Option<Vec<FigValue>>,
    derived: Option<Vec<FigValue>>,
    instance_size: Option<FigVec2>,
    assignment_cache: &mut HashMap<String, String>,
) -> Vec<TreeNode> {
    let mut key = InstanceExpansionKey::new(
        component_guid,
        overrides.unwrap_or_default(),
        derived.unwrap_or_default(),
        instance_size,
        cache.global_assignment_epoch,
        cache.assignment_epoch(component_guid),
    );
    if let Some(children) = cache.get(&key) {
        return children;
    }

    let assignments_before = assignment_cache.len();
    let children = apply_instance_overrides_cached(
        symbol_node,
        Some(&key.overrides),
        Some(&key.derived),
        instance_size,
        assignment_cache,
    );
    // `apply_instance_overrides_cached` inserts only for an unpinned key, so a
    // length increase exactly identifies a newly learned component pin.
    if assignment_cache.len() != assignments_before {
        key.assignment_epoch = cache.bump_assignment_epoch(component_guid);
        key.global_assignment_epoch = cache.global_assignment_epoch;
    }
    cache.insert(key, &children);
    children
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_symbol_and_owned_string_share_exact_cache_equality_and_hash() {
        let owned = FigValue::Str("FUTURE_MODE".into());
        let symbol = FigValue::Symbol("FUTURE_MODE".into());
        assert!(fig_value_equal(&owned, &symbol));
        let (mut left, mut right) = (
            std::collections::hash_map::DefaultHasher::new(),
            std::collections::hash_map::DefaultHasher::new(),
        );
        hash_fig_value(&owned, &mut left);
        hash_fig_value(&symbol, &mut right);
        assert_eq!(left.finish(), right.finish());
        assert!(!fig_value_equal(&owned, &FigValue::Symbol("OTHER".into())));
    }

    #[test]
    fn ledger_counts_exact_backing_spare_outer_capacity_and_shared_keys() {
        let mut text = String::with_capacity(256);
        text.push('x');
        let text_capacity = text.len();
        let mut fields = Vec::with_capacity(128);
        fields.push((
            crate::kiwi::FigKey::from("field"),
            FigValue::Str(text.into()),
        ));
        let expected = std::mem::size_of::<FigValue>()
            + fields.len() * std::mem::size_of::<(crate::kiwi::FigKey, FigValue)>()
            + fields[0].0.retained_bytes()
            + text_capacity;
        let value = FigValue::Object(fields.into());
        assert_eq!(fig_value_bytes(&value), expected);
        let mut array = Vec::with_capacity(64);
        array.push(FigValue::Bool(true));
        let array_capacity = array.len();
        assert_eq!(
            fig_value_bytes(&FigValue::Array(array.into())),
            (array_capacity + 1) * std::mem::size_of::<FigValue>()
        );
        let mut overrides = Vec::with_capacity(32);
        overrides.push(value);
        let key = InstanceExpansionKey::new("1:2", overrides, Vec::new(), None, 0, 0);
        let bound = key.structural_bytes() - 1;
        let mut cache = InstanceExpansionCache::with_limits(4, bound);
        cache.insert(key, &[]);
        assert!(
            cache.entries.is_empty(),
            "an oversized reserved payload is not retained"
        );
        assert_eq!(cache.structural_bytes, 0);
    }

    #[test]
    fn stricter_byte_budget_falls_back_to_identical_expansion() {
        let symbol = symbol();
        let overrides = vec![override_value(1, 10, "renamed")];
        let derived = vec![path(1, 10)];
        let expected = apply_instance_overrides_cached(
            &symbol,
            Some(&overrides),
            Some(&derived),
            None,
            &mut HashMap::new(),
        );
        let mut cache = InstanceExpansionCache::with_limits(4, 1);
        let actual = apply_instance_overrides_memoized(
            &mut cache,
            "0:0",
            &symbol,
            Some(overrides),
            Some(derived),
            None,
            &mut HashMap::new(),
        );
        assert_trees_equal(&actual, &expected);
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn packed_float_cache_keys_preserve_bits_order_independence_and_view_accounting() {
        use crate::kiwi::FloatRecord;
        use std::collections::hash_map::DefaultHasher;
        let values = [f32::from_bits(0x7fc01234), -0.0, 0.25, 0.75];
        let forms = [
            (
                FigValue::Vec2(FloatRecord::new([values[0], values[1]])),
                vec!["x", "y"],
            ),
            (
                FigValue::Color(Box::new(FloatRecord::new(values))),
                vec!["r", "g", "b", "a"],
            ),
        ];
        for (compact, names) in forms {
            let fields = names
                .iter()
                .zip(values)
                .map(|(name, value)| (*name, FigValue::Float(value)))
                .collect::<Vec<_>>();
            let mut reversed = fields.clone();
            reversed.reverse();
            for ordinary in [obj(fields), obj(reversed)] {
                assert!(fig_value_equal(&compact, &ordinary));
                let (mut left, mut right) = (DefaultHasher::new(), DefaultHasher::new());
                hash_fig_value(&compact, &mut left);
                hash_fig_value(&ordinary, &mut right);
                assert_eq!(left.finish(), right.finish());
            }
            let mut positive = compact.clone();
            positive.set(names[1], FigValue::Float(0.0));
            assert!(!fig_value_equal(&compact, &positive));
            let mut other_nan = compact.clone();
            other_nan.set(names[0], FigValue::Float(f32::from_bits(0x7fc05678)));
            assert!(!fig_value_equal(&compact, &other_nan));
            let before = fig_value_bytes(&compact);
            assert!(compact.get(names[0]).is_some());
            assert_eq!(
                fig_value_bytes(&compact) - before,
                names.len() * std::mem::size_of::<FigValue>()
            );
        }
    }

    #[test]
    fn compact_affine_cache_equality_hash_keep_float_bits_and_ignore_field_order() {
        use std::collections::hash_map::DefaultHasher;
        let names = ["m00", "m01", "m02", "m10", "m11", "m12"];
        let values = [1.0, -0.0, f32::from_bits(0x7fc01234), 0.0, 1.0, 2.0];
        let compact = FigValue::Affine(Box::new(values));
        let fields = names
            .into_iter()
            .zip(values)
            .map(|(name, value)| (name, FigValue::Float(value)))
            .collect::<Vec<_>>();
        let mut reversed = fields.clone();
        reversed.reverse();
        for ordinary in [obj(fields), obj(reversed)] {
            assert!(fig_value_equal(&compact, &ordinary));
            let mut left = DefaultHasher::new();
            let mut right = DefaultHasher::new();
            hash_fig_value(&compact, &mut left);
            hash_fig_value(&ordinary, &mut right);
            assert_eq!(left.finish(), right.finish());
        }
        let mut positive = values;
        positive[1] = 0.0;
        assert!(!fig_value_equal(
            &compact,
            &FigValue::Affine(Box::new(positive))
        ));
        let mut other_nan = values;
        other_nan[2] = f32::from_bits(0x7fc05678);
        assert!(!fig_value_equal(
            &compact,
            &FigValue::Affine(Box::new(other_nan))
        ));
    }

    #[test]
    fn compact_and_generic_guid_cache_keys_have_identical_equality_and_hashes() {
        use std::collections::hash_map::DefaultHasher;
        let forms = [
            FigValue::Guid(1, 2),
            guid(1, 2),
            obj(vec![
                ("localID", FigValue::Uint(2)),
                ("sessionID", FigValue::Uint(1)),
            ]),
        ];
        let mut hashes = Vec::new();
        for value in &forms {
            for other in &forms {
                assert!(fig_value_equal(value, other));
            }
            let mut hash = DefaultHasher::new();
            hash_fig_value(value, &mut hash);
            hashes.push(hash.finish());
        }
        assert!(hashes.iter().all(|hash| *hash == hashes[0]));
    }

    fn obj(pairs: Vec<(&str, FigValue)>) -> FigValue {
        FigValue::Object(
            pairs
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        )
    }

    fn guid(session_id: u32, local_id: u32) -> FigValue {
        obj(vec![
            ("sessionID", FigValue::Uint(session_id)),
            ("localID", FigValue::Uint(local_id)),
        ])
    }

    fn size(x: f32, y: f32) -> FigValue {
        obj(vec![("x", FigValue::Float(x)), ("y", FigValue::Float(y))])
    }

    fn path(session_id: u32, local_id: u32) -> FigValue {
        obj(vec![(
            "guidPath",
            obj(vec![(
                "guids",
                FigValue::Array(vec![guid(session_id, local_id)].into()),
            )]),
        )])
    }

    fn override_value(session_id: u32, local_id: u32, name: &str) -> FigValue {
        let mut value = path(session_id, local_id);
        value.set("name", FigValue::Str(name.to_string().into()));
        value
    }

    fn leaf(name: &str, local_id: u32) -> TreeNode {
        TreeNode {
            figma: obj(vec![
                ("type", FigValue::Str("RECTANGLE".into())),
                ("guid", guid(1, local_id)),
                ("name", FigValue::Str(name.into())),
                ("size", size(100.0, 100.0)),
            ]),
            children: Vec::new(),
        }
    }

    fn symbol() -> TreeNode {
        TreeNode {
            figma: obj(vec![
                ("type", FigValue::Str("SYMBOL".into())),
                ("guid", guid(0, 0)),
                ("size", size(100.0, 100.0)),
            ]),
            children: vec![leaf("child", 10)],
        }
    }

    fn assert_trees_equal(left: &[TreeNode], right: &[TreeNode]) {
        assert_eq!(left.len(), right.len());
        for (left, right) in left.iter().zip(right) {
            assert_eq!(left.figma, right.figma);
            assert_trees_equal(&left.children, &right.children);
        }
    }

    fn key(component: &str, name: &str, size: Option<FigVec2>, epoch: u64) -> InstanceExpansionKey {
        InstanceExpansionKey::new(
            component,
            vec![override_value(1, 10, name)],
            vec![path(1, 10)],
            size,
            epoch,
            epoch,
        )
    }

    #[test]
    fn identical_signature_hits_and_returns_an_independent_deep_clone() {
        let symbol = symbol();
        let overrides = vec![override_value(1, 10, "renamed")];
        let derived = vec![path(1, 10)];
        let mut assignments = HashMap::new();
        let expected = apply_instance_overrides_cached(
            &symbol,
            Some(&overrides),
            Some(&derived),
            None,
            &mut HashMap::new(),
        );
        let mut cache = InstanceExpansionCache::default();
        let mut first = apply_instance_overrides_memoized(
            &mut cache,
            "0:0",
            &symbol,
            Some(overrides.clone()),
            Some(derived.clone()),
            None,
            &mut assignments,
        );
        assert_trees_equal(&first, &expected);
        first[0].figma.set("name", FigValue::Str("mutated".into()));
        let second = apply_instance_overrides_memoized(
            &mut cache,
            "0:0",
            &symbol,
            Some(overrides),
            Some(derived),
            None,
            &mut assignments,
        );
        assert_eq!(cache.hits, 1);
        assert_eq!(cache.misses, 1);
        assert_eq!(second[0].figma.get_str("name"), Some("renamed"));
        assert_trees_equal(&second, &expected);
    }

    #[test]
    fn every_structural_key_dimension_causes_a_miss() {
        let mut cache = InstanceExpansionCache::with_limits(16, 1_000_000);
        let base = key("0:0", "base", Some(FigVec2 { x: 100.0, y: 100.0 }), 0);
        cache.insert(base.clone(), &[leaf("cached", 20)]);
        let mut variants = vec![
            key("0:1", "base", Some(FigVec2 { x: 100.0, y: 100.0 }), 0),
            key("0:0", "override", Some(FigVec2 { x: 100.0, y: 100.0 }), 0),
            key("0:0", "base", Some(FigVec2 { x: 101.0, y: 100.0 }), 0),
            key("0:0", "base", Some(FigVec2 { x: 100.0, y: 100.0 }), 1),
        ];
        let mut changed_derived = base.clone();
        changed_derived.derived.push(path(1, 11));
        variants.push(changed_derived);
        let mut changed_global_epoch = base.clone();
        changed_global_epoch.global_assignment_epoch = 1;
        variants.push(changed_global_epoch);
        for variant in variants {
            assert!(cache.get(&variant).is_none());
        }
        assert!(cache.get(&base).is_some());
    }

    #[test]
    fn new_assignment_epoch_invalidates_nested_dependency_entries() {
        let mut cache = InstanceExpansionCache::with_limits(16, 1_000_000);
        let old = key("0:0", "base", None, 0);
        let other = key("0:1", "other", None, 0);
        cache.insert(old.clone(), &[leaf("old", 20)]);
        cache.insert(other.clone(), &[leaf("other", 21)]);
        assert_eq!(cache.bump_assignment_epoch("0:0"), 1);
        assert!(cache.get(&old).is_none());
        assert!(cache.get(&key("0:0", "base", None, 1)).is_none());
        assert!(cache.get(&other).is_none());
    }

    #[test]
    fn fifo_capacity_evicts_the_oldest_exact_key() {
        let mut cache = InstanceExpansionCache::with_limits(2, 1_000_000);
        let first = key("0:0", "first", None, 0);
        let second = key("0:0", "second", None, 0);
        let third = key("0:0", "third", None, 0);
        cache.insert(first.clone(), &[leaf("first", 20)]);
        cache.insert(second.clone(), &[leaf("second", 21)]);
        cache.insert(third.clone(), &[leaf("third", 22)]);
        assert_eq!(cache.entries.len(), 2);
        assert!(cache.get(&first).is_none());
        assert!(cache.get(&second).is_some());
        assert!(cache.get(&third).is_some());
    }
}
