//! Exact direct-child projections from completed inverses, never command replay.
use super::{
    auxiliary,
    patch::{self, Patch, Scope},
    splice_order::{self, Token},
    ScopedEditorSnapshot,
};
use crate::{
    CanonicalScope, CanonicalSplice, CanonicalSpliceTarget, EditorState, PenDocument, PenNodeExt,
};
use jian_ops_schema::node::PenNode;
use std::collections::{HashMap, HashSet};
pub(super) const MAX_IDS: usize = 8192;
const MAX_ID_BYTES: usize = 512 * 1024;
const MAX_DELTAS: usize = 256;
pub(super) fn ids<'a>(values: impl IntoIterator<Item = &'a str>) -> Option<Vec<&'a str>> {
    let mut result = Vec::new();
    let mut bytes = 0usize;
    for id in values {
        bytes = bytes.checked_add(id.len())?;
        if result.len() == MAX_IDS || bytes > MAX_ID_BYTES {
            return None;
        }
        result.push(id);
    }
    Some(result)
}
fn kind(node: &PenNode) -> &'static str {
    match node {
        PenNode::Frame(_) => "frame",
        PenNode::Group(_) => "group",
        PenNode::Rectangle(_) => "rectangle",
        PenNode::Ellipse(_) => "ellipse",
        PenNode::Polygon(_) => "polygon",
        PenNode::Line(_) => "line",
        PenNode::Text(_) => "text",
        PenNode::Path(_) => "path",
        PenNode::TextInput(_) => "text_input",
        PenNode::TextArea(_) => "text_area",
        PenNode::Select(_) => "select",
        PenNode::Switch(_) => "switch",
        PenNode::Checkbox(_) => "checkbox",
        PenNode::Slider(_) => "slider",
        PenNode::RadioGroup(_) => "radio_group",
        PenNode::NumberInput(_) => "number_input",
        PenNode::Progress(_) => "progress",
        PenNode::Tabs(_) => "tabs",
        PenNode::Image(_) => "image",
        PenNode::IconFont(_) => "icon_font",
        PenNode::Ref(_) => "ref",
    }
}
pub(super) fn current<'a>(
    doc: &'a PenDocument,
    scope: &'a Scope,
    path: &'a [usize],
) -> Option<(&'a [PenNode], bool, CanonicalSpliceTarget<'a>)> {
    if path.len() > 128 {
        return None;
    }
    let roots = patch::roots(doc, scope).ok()?;
    let scope_value = match scope {
        Scope::Root => CanonicalScope::Root,
        Scope::Page { index, id } => CanonicalScope::Page {
            index: super::pages::index_hint(doc, *index, id)?,
            id,
        },
    };
    let mut nodes = roots;
    let mut parent = None;
    for at in path {
        let found = nodes.get(*at)?;
        // Ambiguous or virtual ancestry cannot authorize an old raw byte span.
        if matches!(found, PenNode::Ref(_))
            || nodes
                .iter()
                .filter(|node| node.id_str() == found.id_str())
                .count()
                != 1
        {
            return None;
        }
        parent = Some(found);
        nodes = found.children().map_or(&[], Vec::as_slice);
    }
    let present = parent.is_none_or(|node| node.children().is_some());
    Some((
        nodes,
        present,
        CanonicalSpliceTarget {
            scope: scope_value,
            parent_path: path,
            parent_id: parent.map(PenNodeExt::id_str),
            parent_kind: parent.map(kind),
        },
    ))
}
/// Build only a descriptor over borrowed canonical nodes. `reusable` is supplied
/// by an actual ownership/compaction proof, never inferred here from equal IDs.
pub(super) fn list<'a>(
    doc: &'a PenDocument,
    scope: &'a Scope,
    path: &'a [usize],
    before: Vec<&'a str>,
    before_present: bool,
    reusable: Vec<&'a str>,
    force: &HashSet<&str>,
) -> Option<CanonicalSplice<'a>> {
    let (after, after_present, target) = current(doc, scope, path)?;
    if before.len() > MAX_IDS
        || after.len() > MAX_IDS
        || reusable.len() > MAX_IDS * 2
        || !before_present && !before.is_empty()
        || !after_present && !after.is_empty()
        || path.is_empty() && (!before_present || !after_present)
    {
        return None;
    }
    let before_unique = before.iter().copied().collect::<HashSet<_>>();
    let after_unique = after.iter().map(PenNodeExt::id_str).collect::<HashSet<_>>();
    if before_unique.len() != before.len()
        || after_unique.len() != after.len()
        || before
            .iter()
            .copied()
            .chain(after.iter().map(PenNodeExt::id_str))
            .map(str::len)
            .try_fold(0usize, usize::checked_add)?
            > MAX_ID_BYTES * 2
    {
        return None;
    }
    let mut start = 0;
    while start < before.len().min(after.len())
        && before[start] == after[start].id_str()
        && !force.contains(before[start])
    {
        start += 1;
    }
    let mut old_end = before.len();
    let mut new_end = after.len();
    while old_end > start
        && new_end > start
        && before[old_end - 1] == after[new_end - 1].id_str()
        && !force.contains(before[old_end - 1])
    {
        old_end -= 1;
        new_end -= 1;
    }
    Some(CanonicalSplice::new(
        target,
        before_present,
        after_present,
        before,
        after.len(),
        start,
        old_end,
        &after[start..new_end],
        reusable,
    ))
}
struct Budget(usize);
impl Budget {
    fn new(limit: usize) -> Self {
        Self(limit.saturating_mul(4).min(16 * 1024 * 1024))
    }
    fn reserve(&mut self, bytes: usize) -> Option<()> {
        self.0 = self.0.checked_sub(bytes)?;
        Some(())
    }
    fn packet(&mut self, packet: &CanonicalSplice<'_>) -> Option<()> {
        self.reserve(packet.retained_bytes()?.checked_add(256)?)
    }
}
/// Reconstruct the precise pre-command direct order with the SAME anchor graph
/// as rollback, restricted to fresh completed inverses. Historical/rebased
/// branches are deliberately excluded; no history or whole-document census.
fn splices<'a>(
    patches: &'a [Patch],
    doc: &'a PenDocument,
    budget: &mut Budget,
) -> Option<Vec<CanonicalSplice<'a>>> {
    type Key = (Scope, Option<String>);
    let mut groups: Vec<Vec<&super::splice::SplicePatch>> = Vec::new();
    let mut by_key: HashMap<Key, usize> = HashMap::new();
    for splice in patches.iter().flat_map(Patch::splices) {
        if !splice.sealed || !splice.recent.is_empty() {
            return None;
        }
        let bytes = splice
            .expected_ids
            .iter()
            .chain(&splice.logical_ids)
            .map(String::len)
            .sum::<usize>()
            .checked_add(splice.old.iter().map(|old| old.id_str().len()).sum())?
            .checked_add(splice.left.as_ref().map_or(0, String::len))?
            .checked_add(splice.right.as_ref().map_or(0, String::len))?;
        budget.reserve(bytes.checked_mul(4)?.checked_add(1024)?)?;
        let key = (splice.scope.clone(), splice.parent_id.clone());
        let at = match by_key.get(&key) {
            Some(at) => *at,
            None => {
                if groups.len() == MAX_DELTAS {
                    return None;
                }
                let at = groups.len();
                by_key.insert(key, at);
                groups.push(Vec::new());
                at
            }
        };
        groups[at].push(splice);
    }
    let mut packets = Vec::new();
    for group in groups {
        let first = *group.first()?;
        let (after, present, _) = current(doc, &first.scope, &first.parent)?;
        if after.len() > MAX_IDS {
            return None;
        }
        let mut removed = HashSet::new();
        let mut restored = Vec::new();
        let mut anchors = Vec::new();
        for splice in &group {
            if splice.parent.as_ref() != first.parent.as_ref()
                || splice.expected_total != after.len()
                || splice.expected_present != present
                || splice.old_present != first.old_present
            {
                return None;
            }
            let (known, known_present) = splice.current(doc).ok()?;
            if known.as_ptr() != after.as_ptr()
                || known.len() != after.len()
                || known_present != present
            {
                return None;
            }
            let mut previous = None;
            for id in &splice.expected_ids {
                let mut matches = after
                    .iter()
                    .enumerate()
                    .filter(|(_, node)| node.id_str() == id);
                let (at, _) = matches.next()?;
                if matches.next().is_some()
                    || previous.is_some_and(|last| last >= at)
                    || !removed.insert(id.as_str())
                {
                    return None;
                }
                previous = Some(at);
            }
            restored.extend(splice.old.iter().map(|old| old.id_str()));
            anchors.push(splice.anchors(!splice.old.is_empty()));
        }
        if restored.len() > MAX_IDS {
            return None;
        }
        let remaining = after
            .iter()
            .filter(|node| !removed.contains(node.id_str()))
            .collect::<Vec<_>>();
        let before = if restored.is_empty() {
            ids(remaining.iter().map(|node| node.id_str()))?
        } else {
            let existing = remaining
                .iter()
                .map(|node| node.id_str().to_owned())
                .collect::<Vec<_>>();
            let restore = restored
                .iter()
                .map(|id| (*id).to_owned())
                .collect::<Vec<_>>();
            let order = splice_order::order(&existing, &restore, anchors.into_iter()).ok()?;
            let projected = order
                .iter()
                .map(|token| match token {
                    Token::Existing(at) => remaining.get(*at).map(|node| node.id_str()),
                    Token::Restore(at) => restored.get(*at).copied(),
                })
                .collect::<Option<Vec<_>>>()?;
            ids(projected)?
        };
        let reuse = remaining.iter().map(|node| node.id_str()).collect();
        let packet = list(
            doc,
            &first.scope,
            &first.parent,
            before,
            first.old_present,
            reuse,
            &removed,
        )?;
        budget.packet(&packet)?;
        packets.push(packet);
    }
    Some(packets)
}
fn same_target(a: CanonicalSpliceTarget<'_>, b: CanonicalSpliceTarget<'_>) -> bool {
    a.scope == b.scope && a.parent_id == b.parent_id && a.parent_path == b.parent_path
}
impl ScopedEditorSnapshot {
    // Full catalog checkpoints are conservative rollback storage. They may
    // still be proved unchanged without walking owned prototype forests: bound
    // only the shallow exact headers/order/backing, otherwise retain fallback.
    fn canonical_catalog_unchanged(&self, state: &EditorState, budget: &mut Budget) -> bool {
        if !self.captures_components {
            return true;
        }
        let before = self.components.components();
        let current = &state.components.components;
        if before.len() > MAX_IDS || before.len() != current.len() {
            return false;
        }
        let mut charge = self.components.backing_storage_bytes();
        for (old, new) in before.iter().zip(current) {
            if old.id != new.id
                || old.name != new.name
                || old
                    .root
                    .children()
                    .is_some_and(|children| !children.is_empty())
                || new
                    .root
                    .children()
                    .is_some_and(|children| !children.is_empty())
            {
                return false;
            }
            let Some(next) = charge.checked_add(old.root.storage_bytes()) else {
                return false;
            };
            charge = next;
        }
        if budget.reserve(charge).is_none() || !self.components.backing_matches(&state.components) {
            return false;
        }
        before
            .iter()
            .zip(current)
            .all(|(old, new)| old.root.equals_node(&new.root))
    }
    pub(crate) fn visit_canonical_deltas<'a>(
        &'a self,
        state: &'a EditorState,
        limit: usize,
        nodes: &mut impl FnMut(&PenNode, &PenNode),
        headers: &mut impl FnMut(crate::CanonicalHeaderChange) -> bool,
        visitor: &mut impl FnMut(CanonicalSplice<'a>) -> bool,
    ) -> bool {
        if self.page_patch.is_some()
            || self.header.is_some()
            || !self.page_headers.is_empty()
            || self.patches.len() > 4096
            || self.auxiliary_patches.iter().any(|p| {
                matches!(
                    p,
                    auxiliary::Patch::Catalog(_) | auxiliary::Patch::Page { .. }
                )
            })
            || self.validate_shape(state).is_err()
            || auxiliary::Prepared::capture(state, &self.auxiliary_patches, false).is_err()
        {
            return false;
        }
        let mut budget = Budget::new(limit);
        if !self.canonical_catalog_unchanged(state, &mut budget) {
            return false;
        }
        let Some(mut packets) = splices(&self.patches, &state.doc, &mut budget) else {
            return false;
        };
        let mut metadata = Vec::new();
        let mut replaced = Vec::new();
        let mut seen = HashSet::new();
        for edit in &self.patches {
            if edit.validate(&state.doc).is_err() {
                return false;
            }
            match edit {
                Patch::Splice(_) | Patch::Clones(_) => {}
                Patch::Node(p) if p.metadata_only => {
                    if !seen.insert((&p.scope, p.path.as_ref())) {
                        return false;
                    }
                    let Ok(roots) = patch::roots(&state.doc, &p.scope) else {
                        return false;
                    };
                    let Some(current) = patch::node(roots, &p.path) else {
                        return false;
                    };
                    if p.old.children().is_some() != current.children().is_some() {
                        return false;
                    }
                    metadata.push((p, current));
                }
                Patch::Node(p) => {
                    let Some(packet) = p.canonical_replacement(&state.doc) else {
                        return false;
                    };
                    if budget.packet(&packet).is_none() {
                        return false;
                    }
                    replaced.push(p);
                    packets.push(packet);
                }
                Patch::Order(p) => {
                    let Some(packet) = p.canonical_delta(&state.doc) else {
                        return false;
                    };
                    if budget.packet(&packet).is_none() {
                        return false;
                    }
                    packets.push(packet);
                }
                Patch::Topology(p) => {
                    let Some(deltas) = p.canonical_deltas(&state.doc) else {
                        return false;
                    };
                    for packet in deltas {
                        if budget.packet(&packet).is_none() {
                            return false;
                        }
                        packets.push(packet);
                    }
                }
                Patch::Forest(_) => return false,
            }
        }
        if packets.len() > MAX_DELTAS {
            return false;
        }
        // A root replacement owns its entire new body. Other changes inside
        // that body must not accidentally authorize stale descendant reuse.
        for replacement in &replaced {
            if metadata
                .iter()
                .any(|(p, _)| p.scope == replacement.scope && p.path.starts_with(&replacement.path))
            {
                return false;
            }
            for packet in &packets {
                let target = packet.target();
                let same_scope = match (&replacement.scope, target.scope) {
                    (Scope::Root, CanonicalScope::Root) => true,
                    (Scope::Page { id, .. }, CanonicalScope::Page { id: current, .. }) => {
                        id == current
                    }
                    _ => false,
                };
                if same_scope && target.parent_path.starts_with(&replacement.path) {
                    return false;
                }
            }
        }
        for (at, packet) in packets.iter().enumerate() {
            if packets[..at]
                .iter()
                .any(|other| same_target(other.target(), packet.target()))
            {
                return false;
            }
        }
        let excluded = replaced
            .iter()
            .map(|p| p.expected_id.as_str())
            .collect::<HashSet<_>>();
        for packet in &mut packets {
            packet.exclude_reuse(&excluded);
        }
        // Every structural descriptor and local header was validated before
        // exposure. A later visitor/header refusal still discards ALL output.
        for (p, current) in metadata {
            nodes(p.old.metadata(), current);
        }
        if !self
            .auxiliary_patches
            .iter()
            .all(|p| p.visit_canonical_headers(state, limit, headers))
        {
            return false;
        }
        packets.into_iter().all(visitor)
    }
}
