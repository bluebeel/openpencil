//! Actual typed/rich insertion identities, scoped to the applying thread. No
//! document census, selection inference or allocator prediction is involved.
use crate::{history::Scope, EditorCommand, EditorState, NodeId, PenNodeExt};
use jian_ops_schema::{node::PenNode, PenDocument};
use std::{
    cell::{Cell, RefCell},
    marker::PhantomData,
    rc::Rc,
    sync::Weak,
};
const MAX_SCOPES: usize = 16;
const MAX_PATH: usize = 512;
struct Written {
    path: Box<[usize]>,
    id: NodeId,
}
struct Frame {
    ticket: u64,
    owner: Weak<()>,
    generation: u64,
    revision: u64,
    scope: Scope,
    parent: NodeId,
    written: Option<Written>,
    ambiguous: bool,
}
thread_local! {
    static SCOPES: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) };
    static NEXT: Cell<u64> = const { Cell::new(1) };
}
/// Non-Send request evidence. The host retains its existing command/save/undo
/// contract; only a successful actual append can supply a created root ID.
pub struct InsertRootCapture {
    ticket: u64,
    _thread: PhantomData<Rc<()>>,
}
fn request(command: &EditorCommand, depth: usize) -> Option<(&NodeId, Option<&str>)> {
    if depth > 8 {
        return None;
    }
    match command {
        EditorCommand::InsertNode {
            target_parent,
            page_id,
            ..
        } => Some((target_parent, page_id.as_deref())),
        EditorCommand::InsertSubtree {
            nodes,
            parent_id,
            page_id,
        } if nodes.len() == 1 => Some((parent_id, page_id.as_deref())),
        // Rich insert_node hoists actual app-state metadata before its one
        // subtree. Arbitrary mixed/multi-root batches keep their old result.
        EditorCommand::Batch { commands } => match commands.as_slice() {
            [EditorCommand::MergeAppState { .. }, insert] => request(insert, depth + 1),
            [insert] => request(insert, depth + 1),
            _ => None,
        },
        _ => None,
    }
}
fn roots<'a>(doc: &'a PenDocument, scope: &Scope) -> Option<&'a [PenNode]> {
    match scope {
        Scope::Root => Some(&doc.children),
        Scope::Page { index, id } => doc
            .pages
            .as_ref()?
            .get(*index)
            .filter(|page| page.id == *id)
            .map(|page| page.children.as_slice()),
    }
}
fn at<'a>(doc: &'a PenDocument, scope: &Scope, path: &[usize]) -> Option<&'a PenNode> {
    let (slot, parents) = path.split_last()?;
    let mut nodes = roots(doc, scope)?;
    for index in parents {
        nodes = nodes.get(*index)?.children()?.as_slice();
    }
    nodes.get(*slot)
}
impl InsertRootCapture {
    pub fn begin(state: &EditorState, command: &EditorCommand) -> Option<Self> {
        let (parent, page) = request(command, 0)?;
        if parent.as_str().len() > 16384 || page.is_some_and(|page| page.len() > 16384) {
            return None;
        }
        let index = crate::command_apply::helpers::command_page_index(state, page)?;
        let scope = match state.doc.pages.as_ref().filter(|pages| !pages.is_empty()) {
            Some(pages) => {
                let id = &pages.get(index)?.id;
                if id.len() > 16384 {
                    return None;
                }
                Scope::Page {
                    index,
                    id: id.clone(),
                }
            }
            None => Scope::Root,
        };
        SCOPES.with(|scopes| {
            let mut scopes = scopes.borrow_mut();
            if scopes.len() == MAX_SCOPES {
                return None;
            }
            let ticket = NEXT.with(|next| {
                let ticket = next.get();
                next.set(ticket.checked_add(1)?);
                Some(ticket)
            })?;
            scopes.push(Frame {
                ticket,
                owner: state.derived_cache_owner(),
                generation: state.document_generation(),
                revision: state.document_revision(),
                scope,
                parent: parent.clone(),
                written: None,
                ambiguous: false,
            });
            Some(Self {
                ticket,
                _thread: PhantomData,
            })
        })
    }
    /// Return only after the host's accepted apply decision. Transport paths
    /// still hold replies behind their existing successful durable save barrier.
    pub fn finish(self, state: &EditorState, applied: bool) -> Option<NodeId> {
        let frame = self.take()?;
        if !applied
            || frame.ambiguous
            || !frame.owner.ptr_eq(&state.derived_cache_owner())
            || frame.generation != state.document_generation()
            || state.document_revision() <= frame.revision
        {
            return None;
        }
        let written = frame.written?;
        let (_, parents) = written.path.split_last()?;
        if frame.parent.is_real() {
            if at(&state.doc, &frame.scope, parents)?.id_str() != frame.parent.as_str() {
                return None;
            }
        } else if !parents.is_empty() {
            return None;
        }
        at(&state.doc, &frame.scope, &written.path)
            .filter(|node| node.id_str() == written.id.as_str())
            .map(|_| written.id)
    }
    fn take(&self) -> Option<Frame> {
        SCOPES.with(|scopes| {
            let mut scopes = scopes.borrow_mut();
            let index = scopes
                .iter()
                .rposition(|frame| frame.ticket == self.ticket)?;
            Some(scopes.remove(index))
        })
    }
}
impl Drop for InsertRootCapture {
    fn drop(&mut self) {
        let _ = self.take();
    }
}
pub(crate) fn active(state: &EditorState) -> bool {
    SCOPES.with(|scopes| {
        scopes.borrow().iter().rev().any(|frame| {
            frame.generation == state.document_generation()
                && frame.owner.ptr_eq(&state.derived_cache_owner())
        })
    })
}
pub(crate) fn written_id(node: &PenNode) -> Option<NodeId> {
    (node.id_str().len() <= 16384)
        .then(|| NodeId::new_opt(node.id_str()))
        .flatten()
}
/// Called by the real append primitives after they have written the canonical
/// destination. An observed multi-root append cannot masquerade as one root.
pub(crate) fn record_append(
    state: &EditorState,
    scope: &Scope,
    parent_path: &[usize],
    slot: usize,
    id: Option<NodeId>,
) {
    SCOPES.with(|scopes| {
        let mut scopes = scopes.borrow_mut();
        let Some(frame) = scopes.iter_mut().rev().find(|frame| {
            frame.generation == state.document_generation()
                && frame.owner.ptr_eq(&state.derived_cache_owner())
                && &frame.scope == scope
        }) else {
            return;
        };
        let parent_matches = if frame.parent.is_real() {
            at(&state.doc, scope, parent_path)
                .is_some_and(|parent| parent.id_str() == frame.parent.as_str())
        } else {
            parent_path.is_empty()
        };
        if !parent_matches {
            return;
        }
        let Some(id) = id.filter(|id| id.is_real() && id.as_str().len() <= 16384) else {
            frame.ambiguous = true;
            return;
        };
        if frame.written.is_some() || parent_path.len() >= MAX_PATH {
            frame.ambiguous = true;
            return;
        }
        let mut path = Vec::with_capacity(parent_path.len() + 1);
        path.extend_from_slice(parent_path);
        path.push(slot);
        if !at(&state.doc, scope, &path).is_some_and(|node| node.id_str() == id.as_str()) {
            frame.ambiguous = true;
            return;
        }
        frame.written = Some(Written {
            path: path.into_boxed_slice(),
            id,
        });
    })
}
