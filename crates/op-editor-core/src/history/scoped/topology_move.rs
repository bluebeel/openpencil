//! Reparent/reorder history exchanges owned nodes; descendants are never cloned.
use super::{
    patch,
    topology_parent::{self as parent, Location, Parent},
    ScopedHistoryError,
};
use jian_ops_schema::PenDocument;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Boundary {
    left: Option<String>,
    right: Option<String>,
}
fn boundary(
    nodes: &[jian_ops_schema::node::PenNode],
    index: usize,
    skip: Option<usize>,
) -> Result<Boundary, ScopedHistoryError> {
    let at = |index: usize| nodes.get(index + usize::from(skip.is_some_and(|skip| index >= skip)));
    Ok(Boundary {
        left: parent::identity(index.checked_sub(1).and_then(at))?,
        right: parent::identity(at(index))?,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct MovePatch {
    pub scope: patch::Scope,
    from: Parent,
    to: Parent,
    from_index: usize,
    to_index: usize,
    node_id: String,
    from_total: usize,
    to_total: usize,
    from_present: bool,
    to_present: bool,
    from_boundary: Boundary,
    to_boundary: Boundary,
    restore_from_present: bool,
}
#[derive(Debug)]
pub(super) struct PendingMove {
    before: MovePatch,
    after: Option<MovePatch>,
}
impl MovePatch {
    fn capture(
        doc: &PenDocument,
        scope: patch::Scope,
        from: Location,
        to: Location,
        node_id: &str,
        restore_from_present: bool,
        restore_to_present: bool,
    ) -> Result<Self, ScopedHistoryError> {
        let from_parent = Parent::capture(doc, &scope, from.parent)?;
        let to_parent = Parent::capture(doc, &scope, to.parent)?;
        let (source, from_present) = from_parent.list(doc, &scope)?;
        let (target, to_present) = to_parent.list(doc, &scope)?;
        if source
            .get(from.index)
            .is_none_or(|node| crate::PenNodeExt::id_str(node) != node_id)
            || node_id.len() > 16 * 1024
        {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        let same = from_parent.path == to_parent.path;
        if to.index > target.len() - usize::from(same)
            || !restore_to_present
            || (!restore_from_present && (same || source.len() != 1 || from_parent.path.is_empty()))
        {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        let patch = Self {
            scope,
            from: from_parent,
            to: to_parent,
            from_index: from.index,
            to_index: to.index,
            node_id: node_id.to_owned(),
            from_total: source.len(),
            to_total: target.len(),
            from_present,
            to_present,
            from_boundary: boundary(source, from.index, Some(from.index))?,
            to_boundary: boundary(target, to.index, same.then_some(from.index))?,
            restore_from_present,
        };
        patch.after_routes()?;
        Ok(patch)
    }
    fn after_routes(&self) -> Result<(Location, Location), ScopedHistoryError> {
        let mut to = self.to.path.clone();
        parent::removed(&mut to, &self.from.path, self.from_index)?;
        let mut from = self.from.path.clone();
        parent::inserted(&mut from, &to, self.to_index);
        Ok((
            Location {
                parent: to,
                index: self.to_index,
            },
            Location {
                parent: from,
                index: self.from_index,
            },
        ))
    }
    pub(super) fn validate(&self, doc: &PenDocument) -> Result<(), ScopedHistoryError> {
        let (source, present) = self.from.list(doc, &self.scope)?;
        let (target, target_present) = self.to.list(doc, &self.scope)?;
        let same = self.from.path == self.to.path;
        if source.len() != self.from_total
            || target.len() != self.to_total
            || present != self.from_present
            || target_present != self.to_present
            || source
                .get(self.from_index)
                .is_none_or(|node| crate::PenNodeExt::id_str(node) != self.node_id)
            || self.to_index > target.len() - usize::from(same)
            || boundary(source, self.from_index, Some(self.from_index))? != self.from_boundary
            || boundary(target, self.to_index, same.then_some(self.from_index))? != self.to_boundary
        {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        self.after_routes()?;
        Ok(())
    }
    pub(super) fn exchange(&mut self, doc: &mut PenDocument) {
        let (reverse_from, reverse_to) = self.after_routes().unwrap();
        let node = parent::list_mut(doc, &self.scope, &self.from.path).remove(self.from_index);
        parent::restore_absence(doc, &self.scope, &self.from.path, self.restore_from_present);
        parent::list_mut(doc, &self.scope, &reverse_from.parent).insert(self.to_index, node);
        let reverse = Self::capture(
            doc,
            self.scope.clone(),
            reverse_from,
            reverse_to,
            &self.node_id,
            self.to_present,
            self.from_present,
        )
        .expect("validated topology move retains exact inverse routes");
        *self = reverse;
    }
    pub(super) fn actor_regions(&self) -> [Option<super::actor_claim::Region<'_>>; 2] {
        use super::actor_claim::{Kind, Region};
        [
            Some(Region {
                scope: &self.scope,
                path: &self.from.path,
                kind: Kind::Children,
            }),
            Some(Region {
                scope: &self.scope,
                path: &self.to.path,
                kind: Kind::Children,
            }),
        ]
    }
    pub(super) fn rebase(&mut self, reorders: &patch::Reorders) {
        reorders.rebase(&self.scope, &mut self.from.path);
        reorders.rebase(&self.scope, &mut self.to.path);
    }
    pub(super) fn bytes(&self) -> usize {
        parent::scope_bytes(&self.scope)
            + self.from.bytes()
            + self.to.bytes()
            + self.node_id.capacity()
            + self.from_boundary.left.as_ref().map_or(0, String::capacity)
            + self
                .from_boundary
                .right
                .as_ref()
                .map_or(0, String::capacity)
            + self.to_boundary.left.as_ref().map_or(0, String::capacity)
            + self.to_boundary.right.as_ref().map_or(0, String::capacity)
    }
}
impl PendingMove {
    pub(super) fn capture(
        doc: &PenDocument,
        scope: patch::Scope,
        from: Location,
        to: Location,
        node_id: &str,
    ) -> Result<Self, ScopedHistoryError> {
        Ok(Self {
            before: MovePatch::capture(doc, scope, from, to, node_id, true, true)?,
            after: None,
        })
    }
    pub(super) fn matches(&self, doc: &PenDocument) -> bool {
        self.before.validate(doc).is_ok()
    }
    pub(super) fn apply_direct(&mut self, doc: &mut PenDocument, no_op_is_success: bool) -> bool {
        if self.after.is_some() || self.before.validate(doc).is_err() {
            return false;
        }
        if !no_op_is_success
            && self.before.from.path == self.before.to.path
            && self.before.from_index == self.before.to_index
        {
            return false;
        }
        let mut forward = self.before.clone();
        forward.exchange(doc);
        self.after = Some(forward);
        true
    }
    pub(super) fn compact(
        mut self,
        doc: &PenDocument,
    ) -> Result<Option<MovePatch>, ScopedHistoryError> {
        if let Some(patch) = self.after.take() {
            patch.validate(doc)?;
            return Ok(Some(patch));
        }
        if self.matches(doc) {
            return Ok(None);
        }
        let (from, to) = self.before.after_routes()?;
        let reverse = MovePatch::capture(
            doc,
            self.before.scope.clone(),
            from,
            to,
            &self.before.node_id,
            self.before.to_present,
            self.before.from_present,
        )?;
        let same = self.before.from.path == self.before.to.path;
        let expected_from = if same {
            self.before.to_total
        } else {
            self.before.to_total + 1
        };
        let expected_to = if same {
            self.before.from_total
        } else {
            self.before.from_total - 1
        };
        if reverse.from_total != expected_from
            || reverse.to_total != expected_to
            || reverse.from_boundary != self.before.to_boundary
            || reverse.to_boundary != self.before.from_boundary
        {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        Ok(Some(reverse))
    }
}
