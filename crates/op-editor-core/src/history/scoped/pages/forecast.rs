//! Admission before intended clone payloads or catalog receipts are allocated.
use crate::{
    history_snapshot::{command_capture_node_bytes, command_capture_value_bytes},
    EditorCommand as C, EditorState, PenNodeExt,
};
use jian_ops_schema::{node::PenNode, page::PenPage};
use serde::Serialize;
use std::{collections::HashSet, mem};

#[derive(Serialize)]
struct Header<'a> {
    id: &'a String,
    name: &'a String,
    background_color: &'a Option<String>,
    state: &'a Option<jian_ops_schema::state::StateSchema>,
    lifecycle: &'a Option<jian_ops_schema::lifecycle::PageLifecycleHooks>,
}
pub(super) fn page_header(page: &PenPage, limit: usize) -> Option<usize> {
    let PenPage {
        id,
        name,
        background_color,
        state,
        lifecycle,
        children: _,
    } = page;
    command_capture_value_bytes(
        &Header {
            id,
            name,
            background_color,
            state,
            lifecycle,
        },
        limit,
    )
    .ok()
}
pub(super) fn forest(nodes: &[PenNode], limit: usize) -> Option<(usize, usize)> {
    let mut bytes = 0usize;
    let mut count = 0usize;
    let mut pending = vec![nodes.iter()];
    while let Some(siblings) = pending.last_mut() {
        if let Some(node) = siblings.next() {
            count = count.checked_add(1)?;
            bytes = bytes.checked_add(
                command_capture_node_bytes(node, true, limit.checked_sub(bytes)?).ok()?,
            )?;
            // Remap tables, collision sets, COW SharedNode wrappers, and ID
            // strings must fit too; no source-sized JSON buffer is created.
            bytes =
                bytes.checked_add(1024usize.checked_add(node.id_str().len().checked_mul(32)?)?)?;
            if let Some(children) = node.children() {
                bytes = bytes.checked_add(
                    children
                        .capacity()
                        .checked_mul(mem::size_of::<PenNode>() + 64)?,
                )?;
                if bytes > limit {
                    return None;
                }
                pending.push(children.iter());
            }
        } else {
            pending.pop();
        }
    }
    Some((bytes, count))
}
#[derive(Clone, Copy)]
pub(crate) struct Forecast {
    pub(super) total: usize,
    pub(super) catalogue: usize,
}
impl Forecast {
    pub(crate) fn bytes(self) -> usize {
        self.total
    }
}
pub(super) fn command(state: &EditorState, command: &C, limit: usize) -> Option<Forecast> {
    let ids = super::exchange::ids(state).ok()?;
    let pages = state.doc.pages.as_ref().filter(|pages| !pages.is_empty());
    let mut bytes = 128usize * 1024;
    bytes = bytes.checked_add(
        state
            .doc
            .pages
            .as_ref()
            .map_or(0, Vec::capacity)
            .checked_mul(mem::size_of::<PenPage>() * 4 + 1024)?,
    )?;
    for id in &ids {
        bytes = bytes.checked_add(id.len().checked_mul(32)?)?;
    }
    let (source, incoming, name, removes, duplicate) = match command {
        C::AddPage { name, children } => {
            if name.as_ref().is_some_and(|name| name.trim().is_empty()) {
                return None;
            }
            (None, children.as_deref(), name.as_deref(), false, false)
        }
        C::DuplicatePage { index, name } => {
            if name.as_ref().is_some_and(|name| name.trim().is_empty()) {
                return None;
            }
            let index = *index as usize;
            let source = match pages {
                Some(pages) => Some(pages.get(index)?),
                None if index == 0 => None,
                _ => return None,
            };
            (
                source,
                Some(source.map_or(state.doc.children.as_slice(), |page| {
                    page.children.as_slice()
                })),
                name.as_deref(),
                false,
                true,
            )
        }
        C::DeletePage { index } => {
            let pages = pages?;
            if pages.len() <= 1 {
                return None;
            }
            let source = pages.get(*index as usize)?;
            (
                Some(source),
                Some(source.children.as_slice()),
                None,
                true,
                false,
            )
        }
        C::ReorderPage { from, to } => {
            let pages = pages?;
            if *from as usize >= pages.len()
                || *from as usize == (*to as usize).min(pages.len() - 1)
            {
                return None;
            }
            return (bytes <= limit).then_some(Forecast {
                total: bytes,
                catalogue: 0,
            });
        }
        _ => return None,
    };
    if let Some(name) = name {
        bytes = bytes.checked_add(name.len().checked_mul(8)?)?;
    }
    if let Some(source) = source {
        bytes =
            bytes.checked_add(page_header(source, limit.checked_sub(bytes)?)?.checked_mul(4)?)?;
    }
    let (payload, _) = forest(incoming.unwrap_or(&[]), limit.checked_sub(bytes)?)?;
    bytes = bytes.checked_add(payload.checked_mul(if removes { 2 } else { 4 })?)?;
    let mut selected_ids = HashSet::new();
    let mut reusable = false;
    let mut pending = vec![incoming.unwrap_or(&[]).iter()];
    while let Some(nodes) = pending.last_mut() {
        if let Some(node) = nodes.next() {
            selected_ids.insert(node.id_str());
            reusable |= matches!(node, PenNode::Frame(frame) if frame.reusable == Some(true));
            if let Some(children) = node.children() {
                pending.push(children.iter());
            }
        } else {
            pending.pop();
        }
    }
    let mut catalogue = 0usize;
    if removes || duplicate || reusable {
        catalogue = (128usize * 1024).checked_add(payload.checked_mul(2)?)?;
        for component in &state.components.components {
            catalogue = catalogue.checked_add(
                mem::size_of::<crate::NodeId>()
                    + 128
                    + component.id.as_str().len().checked_mul(8)?,
            )?;
            if selected_ids.contains(component.id.as_str()) {
                catalogue = catalogue.checked_add(component.name.len().checked_mul(16)?)?;
                if removes && state.components.is_document_backed(&component.id) {
                    catalogue = catalogue.checked_add(
                        forest(
                            std::slice::from_ref(&component.root),
                            limit.checked_sub(catalogue)?,
                        )?
                        .0
                        .checked_mul(4)?,
                    )?;
                }
            }
        }
        let backing = state.components.document_backing();
        catalogue = catalogue.checked_add(backing.capacity().checked_mul(
            mem::size_of::<(
                crate::NodeId,
                crate::component_backing::DocumentNodeLocation,
            )>() + 64,
        )?)?;
        for (id, location) in backing.iter() {
            catalogue = catalogue
                .checked_add(id.as_str().len().checked_mul(8)?)?
                .checked_add(
                    location
                        .child_path
                        .len()
                        .checked_mul(mem::size_of::<usize>() * 8)?,
                )?
                .checked_add(
                    location
                        .page_id
                        .as_ref()
                        .map_or(0, |id| 64 + id.len().saturating_mul(8)),
                )?;
        }
        bytes = bytes.checked_add(catalogue)?;
    }
    // Legacy promotion is a real derived-map transition. Reserve its COW map
    // and per-key old/expected paths, never a whole catalog prototype snapshot.
    if pages.is_none() {
        let backing = state.components.document_backing();
        bytes = bytes.checked_add(backing.capacity().checked_mul(
            mem::size_of::<(
                crate::NodeId,
                crate::component_backing::DocumentNodeLocation,
            )>() + 64,
        )?)?;
        for (id, location) in backing.iter() {
            bytes = bytes
                .checked_add(id.as_str().len().checked_mul(8)?)?
                .checked_add(
                    location
                        .child_path
                        .len()
                        .checked_mul(mem::size_of::<usize>() * 8)?,
                )?;
        }
    }
    (bytes <= limit).then_some(Forecast {
        total: bytes,
        catalogue,
    })
}
