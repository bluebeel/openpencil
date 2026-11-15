//! Request-local evidence from the actual detach write, never ID prediction.
use crate::{EditorCommand, EditorState, NodeId, PenNodeExt};
use std::{
    cell::{Cell, RefCell},
    marker::PhantomData,
    rc::Rc,
    sync::Weak,
};
const MAX_SCOPES: usize = 16;
const MAX_ID: usize = 16 * 1024;
#[derive(Clone)]
enum Scope {
    Root,
    Page { hint: usize, id: String },
}
struct Written {
    scope: Scope,
    path: Box<[usize]>,
    node_id: NodeId,
    created_root: bool,
}
struct Frame {
    ticket: u64,
    owner: Weak<()>,
    generation: u64,
    revision: u64,
    source: NodeId,
    written: Option<Written>,
    ambiguous: bool,
}
thread_local! {
    static SCOPES:RefCell<Vec<Frame>>=const {RefCell::new(Vec::new())};
    static NEXT:Cell<u64>=const {Cell::new(1)};
}
/// Actual surviving identity. newRootIds describes only new roots, not every
/// descendant reminted while materializing an instance.
pub struct ComponentDetachResult {
    pub node_id: NodeId,
    pub created_root: bool,
}
/// Lexical, non-Send capture on the actual authority/commit executing thread.
pub struct ComponentDetachCapture {
    ticket: u64,
    _thread: PhantomData<Rc<()>>,
}
impl ComponentDetachCapture {
    pub fn begin(state: &EditorState, command: &EditorCommand) -> Option<Self> {
        let EditorCommand::DetachComponent { node_id } = command else {
            return None;
        };
        if node_id.as_str().len() > MAX_ID {
            return None;
        }
        SCOPES.with(|slot| {
            let mut scopes = slot.borrow_mut();
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
                source: node_id.clone(),
                written: None,
                ambiguous: false,
            });
            Some(Self {
                ticket,
                _thread: PhantomData,
            })
        })
    }
    /// The host calls this only after its real apply/save decision. Cohorts
    /// retain this reply privately until their final durable commit succeeds.
    pub fn finish(self, state: &EditorState, applied: bool) -> Option<ComponentDetachResult> {
        let frame = self.take()?;
        if !applied
            || frame.ambiguous
            || frame.generation != state.document_generation()
            || !frame.owner.ptr_eq(&state.derived_cache_owner())
            || frame.revision == state.document_revision()
        {
            return None;
        }
        let written = frame.written?;
        let mut roots = match &written.scope {
            Scope::Root => {
                if state
                    .doc
                    .pages
                    .as_ref()
                    .is_some_and(|pages| !pages.is_empty())
                {
                    return None;
                }
                state.doc.children.as_slice()
            }
            Scope::Page { hint, id } => {
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
                page.children.as_slice()
            }
        };
        let (last, parents) = written.path.split_last()?;
        for at in parents {
            roots = roots.get(*at)?.children()?.as_slice();
        }
        if roots.get(*last)?.id_str() != written.node_id.as_str() {
            return None;
        }
        Some(ComponentDetachResult {
            node_id: written.node_id,
            created_root: written.created_root,
        })
    }
    fn take(&self) -> Option<Frame> {
        SCOPES.with(|slot| {
            let mut scopes = slot.borrow_mut();
            let at = scopes
                .iter()
                .rposition(|frame| frame.ticket == self.ticket)?;
            Some(scopes.remove(at))
        })
    }
}
impl Drop for ComponentDetachCapture {
    fn drop(&mut self) {
        let _ = self.take();
    }
}
/// Called only after the exact slot replacement/definition unregister and its
/// normal history finalization. Allocation failures cannot reach this hook.
pub(crate) fn record(
    state: &EditorState,
    source: &NodeId,
    path: &[usize],
    node_id: &NodeId,
    created_root: bool,
) {
    SCOPES.with(|slot| {
        let mut scopes = slot.borrow_mut();
        let Some(frame) = scopes.iter_mut().rev().find(|frame| {
            frame.source == *source
                && frame.generation == state.document_generation()
                && frame.owner.ptr_eq(&state.derived_cache_owner())
        }) else {
            return;
        };
        if frame.written.is_some() || path.len() > 4096 || node_id.as_str().len() > MAX_ID {
            frame.ambiguous = true;
            return;
        }
        let scope = match state.doc.pages.as_ref().filter(|pages| !pages.is_empty()) {
            Some(pages) => {
                let hint = state.ui.active_page_index.min(pages.len() - 1);
                if pages[hint].id.len() > MAX_ID {
                    frame.ambiguous = true;
                    return;
                }
                Scope::Page {
                    hint,
                    id: pages[hint].id.clone(),
                }
            }
            None => Scope::Root,
        };
        frame.written = Some(Written {
            scope,
            path: path.into(),
            node_id: node_id.clone(),
            created_root,
        });
    });
}
