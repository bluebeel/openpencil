//! Opt-in, lexical capture of actual kit-root writes. No selection/allocator
//! prediction, document census or globally retained EditorState is involved.
use crate::{EditorCommand, EditorState, NodeId, PenNodeExt};
use std::{
    cell::{Cell, RefCell},
    marker::PhantomData,
    rc::Rc,
    sync::Weak,
};
const MAX_SCOPES: usize = 16;
#[derive(Clone)]
enum RootScope {
    Legacy,
    Page { hint: usize, id: String },
}
struct Written {
    scope: RootScope,
    index: usize,
    id: NodeId,
}
struct Frame {
    ticket: u64,
    owner: Weak<()>,
    generation: u64,
    revision: u64,
    kit: String,
    component: String,
    written: Option<Written>,
    ambiguous: bool,
}
thread_local! {
    static SCOPES:RefCell<Vec<Frame>>=const{RefCell::new(Vec::new())};
    static NEXT:Cell<u64>=const{Cell::new(1)};
}
/// Non-Send guard: the real mutator records on this command's executing thread,
/// including a commit/cohort leader. Unwinding drops all request-local evidence.
pub struct KitRootCapture {
    ticket: u64,
    _thread: PhantomData<Rc<()>>,
}
impl KitRootCapture {
    /// Direct root-level kit commands only. Dynamic insert_<component> tools
    /// use this shape; arbitrary parent/callback shapes keep descriptor replies.
    pub fn begin(state: &EditorState, command: &EditorCommand) -> Option<Self> {
        let EditorCommand::InstantiateKitComponent {
            kit_id,
            component_id,
            target_parent,
            ..
        } = command
        else {
            return None;
        };
        if target_parent.is_real() || kit_id.len() > 16384 || component_id.len() > 16384 {
            return None;
        }
        SCOPES.with(|scopes| {
            let mut scopes = scopes.borrow_mut();
            if scopes.len() >= MAX_SCOPES {
                return None;
            }
            let ticket = NEXT.with(|next| {
                let value = next.get();
                next.set(value.checked_add(1)?);
                Some(value)
            })?;
            scopes.push(Frame {
                ticket,
                owner: state.derived_cache_owner(),
                generation: state.document_generation(),
                revision: state.document_revision(),
                kit: kit_id.clone(),
                component: component_id.clone(),
                written: None,
                ambiguous: false,
            });
            Some(Self {
                ticket,
                _thread: PhantomData,
            })
        })
    }
    /// Call only after the host's ordinary apply/save decision. A failed or
    /// rolled-back request never publishes a created identity. The exact write
    /// slot is checked in constant node work, independent of live selection.
    pub fn finish(self, state: &EditorState, applied: bool) -> Option<NodeId> {
        let frame = self.take()?;
        if !applied
            || frame.ambiguous
            || !frame.owner.ptr_eq(&state.derived_cache_owner())
            || frame.generation != state.document_generation()
            || frame.revision == state.document_revision()
        {
            return None;
        }
        let written = frame.written?;
        let roots = match &written.scope {
            RootScope::Legacy => {
                if state
                    .doc
                    .pages
                    .as_ref()
                    .is_some_and(|pages| !pages.is_empty())
                {
                    return None;
                }
                &state.doc.children
            }
            RootScope::Page { hint, id } => {
                let pages = state.doc.pages.as_ref()?;
                let page = if let Some(page) = pages.get(*hint).filter(|page| page.id == *id) {
                    page
                } else {
                    let mut matches = pages.iter().filter(|page| page.id == *id);
                    let page = matches.next()?;
                    if matches.next().is_some() {
                        return None;
                    }
                    page
                };
                &page.children
            }
        };
        roots
            .get(written.index)
            .filter(|node| node.id_str() == written.id.as_str())
            .map(|_| written.id)
    }
    fn take(&self) -> Option<Frame> {
        SCOPES.with(|scopes| {
            let mut scopes = scopes.borrow_mut();
            let at = scopes
                .iter()
                .rposition(|frame| frame.ticket == self.ticket)?;
            Some(scopes.remove(at))
        })
    }
}
impl Drop for KitRootCapture {
    fn drop(&mut self) {
        let _ = self.take();
    }
}
/// Invoked only after the actual append and undo/checkpoint write completed.
/// A callback that never applied a real kit mutation has no receipt to expose.
pub(crate) fn record_root(
    state: &EditorState,
    kit: &str,
    component: &str,
    index: usize,
    id: &NodeId,
) {
    if !id.is_real() {
        return;
    }
    SCOPES.with(|scopes| {
        let mut scopes = scopes.borrow_mut();
        let Some(frame) = scopes.iter_mut().rev().find(|frame| {
            frame.kit == kit
                && frame.component == component
                && frame.generation == state.document_generation()
                && frame.owner.ptr_eq(&state.derived_cache_owner())
        }) else {
            return;
        };
        if frame.written.is_some() {
            frame.ambiguous = true;
            return;
        }
        let scope = match state.doc.pages.as_ref().filter(|pages| !pages.is_empty()) {
            Some(pages) => {
                let hint = state.ui.active_page_index.min(pages.len() - 1);
                RootScope::Page {
                    hint,
                    id: pages[hint].id.clone(),
                }
            }
            None => RootScope::Legacy,
        };
        frame.written = Some(Written {
            scope,
            index,
            id: id.clone(),
        });
    });
}
