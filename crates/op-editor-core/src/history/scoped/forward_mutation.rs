//! Execute wire STATE replacements, validating each against the exact current
//! intermediate state. Caller retains/absorbs moved before-values for rollback.

use super::{
    forward_regions::{self, Before, Data},
    patch::{self, Scope},
};
use crate::{
    web_sync::{
        committed::{Operation, Page},
        CommittedFeedError as Error,
    },
    PenDocument, PenNodeExt,
};

pub(super) fn scope(page: Option<&Page>) -> Scope {
    match page {
        None => Scope::Root,
        Some(page) => Scope::Page {
            index: page.index,
            id: page.id.clone(),
        },
    }
}
fn unique_ids<I, S>(ids: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut seen = std::collections::HashSet::new();
    ids.into_iter()
        .all(|id| seen.insert(id.as_ref().to_owned()))
}
fn parent_path(doc: &PenDocument, scope: &Scope, ids: &[String]) -> Result<Vec<usize>, Error> {
    let mut nodes = patch::roots(doc, scope).map_err(|_| Error::Precondition)?;
    let mut path = Vec::with_capacity(ids.len());
    for id in ids {
        let mut matches = nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.id_str() == id);
        let (at, node) = matches.next().ok_or(Error::Precondition)?;
        if matches.next().is_some() {
            return Err(Error::Precondition);
        }
        path.push(at);
        nodes = node.children().map_or(&[], Vec::as_slice);
    }
    Ok(path)
}
pub(super) fn apply(doc: &mut PenDocument, operation: Operation) -> Result<Before, Error> {
    let scope = scope(operation.page());
    match operation {
        Operation::Metadata(value) => {
            let roots = patch::roots_mut(doc, &scope).map_err(|_| Error::Precondition)?;
            let current = patch::node_mut(roots, &value.path).ok_or(Error::Precondition)?;
            if current.id_str() != value.expected_id
                || std::mem::discriminant(current) != std::mem::discriminant(&value.value)
                || current.children().is_some() != value.children_present
                || value.value.children().is_some() != value.children_present
                || value
                    .value
                    .children()
                    .is_some_and(|children| !children.is_empty())
            {
                return Err(Error::Precondition);
            }
            let old = forward_regions::replace_header(current, value.value);
            Ok(Before {
                scope,
                path: value.path,
                data: Data::Header(old),
            })
        }
        Operation::Node(value) => {
            let roots = patch::roots_mut(doc, &scope).map_err(|_| Error::Precondition)?;
            let current = patch::node_mut(roots, &value.path).ok_or(Error::Precondition)?;
            if current.id_str() != value.expected_id {
                return Err(Error::Precondition);
            }
            let old = std::mem::replace(current, value.value);
            Ok(Before {
                scope,
                path: value.path,
                data: Data::Node(old),
            })
        }
        Operation::Children(value) => {
            let path = parent_path(doc, &scope, &value.parent_ids)?;
            let roots = patch::roots_mut(doc, &scope).map_err(|_| Error::Precondition)?;
            let (current, present) = if path.is_empty() {
                (&roots[..], true)
            } else {
                let parent = patch::node(roots, &path).ok_or(Error::Precondition)?;
                if !parent.is_container() {
                    return Err(Error::Precondition);
                }
                (
                    parent.children().map_or(&[][..], Vec::as_slice),
                    parent.children().is_some(),
                )
            };
            if present != value.expected_present
                || (!present && !value.expected_ids.is_empty())
                || (!path.is_empty() && value.parent_ids.len() != path.len())
                || (path.is_empty() && (!value.expected_present || value.value.is_none()))
                || !unique_ids(&value.expected_ids)
                || !unique_ids(
                    value
                        .value
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .map(PenNodeExt::id_str),
                )
                || current
                    .iter()
                    .map(PenNodeExt::id_str)
                    .ne(value.expected_ids.iter().map(String::as_str))
            {
                return Err(Error::Precondition);
            }
            let old = if path.is_empty() {
                Some(std::mem::replace(roots, value.value.unwrap()))
            } else {
                let parent = patch::node_mut(roots, &path).unwrap();
                let slot = patch::children_slot(parent).ok_or(Error::Precondition)?;
                std::mem::replace(slot, value.value)
            };
            Ok(Before {
                scope,
                path,
                data: Data::Children(old),
            })
        }
    }
}

pub(super) fn compact(before: Vec<Before>, doc: &PenDocument) -> Vec<patch::Patch> {
    let mut patches = Vec::new();
    for before in before {
        let roots = patch::roots(doc, &before.scope).expect("validated feed scope");
        let mut path = before.path;
        match before.data {
            Data::Header(old) => patch::compact_owned_header(
                old,
                patch::node(roots, &path).unwrap(),
                &before.scope,
                &path,
                &mut patches,
            ),
            Data::Node(old) => patch::compact_owned_node(
                old,
                patch::node(roots, &path).unwrap(),
                &before.scope,
                &mut path,
                &mut patches,
            ),
            Data::Children(Some(old)) => {
                let (current, id) = if path.is_empty() {
                    (roots, None)
                } else {
                    let parent = patch::node(roots, &path).unwrap();
                    (parent.children().unwrap().as_slice(), Some(parent.id_str()))
                };
                patch::compact_owned_list(old, current, &before.scope, &mut path, id, &mut patches);
            }
            Data::Children(None) => {} // Both sides absent after normalization.
        }
    }
    patches
}
