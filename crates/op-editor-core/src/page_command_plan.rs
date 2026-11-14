//! Intended page payloads only; every allocation succeeds before a live write.
use super::{
    allocation, make_blank_page_frame, make_page, EditorState, IdAllocError, IdAllocator, PenPage,
};
use crate::EditorCommand as C;

pub(crate) struct PreparedPageCommand(Kind);
enum Kind {
    Insert {
        promotion: allocation::Promotion,
        page: PenPage,
        at: usize,
        legacy: Option<bool>,
        source: Option<usize>,
    },
    Delete {
        index: usize,
    },
    Move {
        from: usize,
        to: usize,
    },
}
pub(crate) enum CommittedPageCommand {
    Insert {
        id: String,
        at: usize,
        legacy: Option<bool>,
    },
    Delete {
        page: PenPage,
        index: usize,
    },
    Move {
        id: String,
        from: usize,
    },
}
impl PreparedPageCommand {
    pub(crate) fn new(
        state: &EditorState,
        command: C,
        allocator: &mut dyn IdAllocator,
    ) -> Result<Option<Self>, IdAllocError> {
        let explicit = state.doc.pages.as_ref().filter(|pages| !pages.is_empty());
        let legacy = explicit.is_none().then_some(state.doc.pages.is_some());
        let result = match command {
            C::AddPage { name, children } => {
                if name.as_ref().is_some_and(|name| name.trim().is_empty()) {
                    return Ok(None);
                }
                let mut taken = allocator.document_collision_ids(&state.doc);
                let promotion = allocation::Promotion::prepare(state, allocator, &mut taken)?;
                let id = allocator.allocate(&mut taken)?;
                let children = match children {
                    Some(mut children) => {
                        super::remap_subtree_ids_with_allocator(
                            &mut children,
                            allocator,
                            &mut taken,
                        )?;
                        children
                    }
                    None => {
                        let frame = allocator.allocate(&mut taken)?;
                        let Some(frame) = make_blank_page_frame(&frame) else {
                            return Ok(None);
                        };
                        vec![frame]
                    }
                };
                let at = explicit.map_or(1, Vec::len);
                let name = name.unwrap_or_else(|| format!("Page {}", at + 1));
                Ok(Some(Kind::Insert {
                    promotion,
                    page: make_page(id.into(), name, children),
                    at,
                    legacy,
                    source: None,
                }))
            }
            C::DuplicatePage { index, name } => {
                if name.as_ref().is_some_and(|name| name.trim().is_empty()) {
                    return Ok(None);
                }
                let index = index as usize;
                if explicit.map_or(index != 0, |pages| index >= pages.len()) {
                    return Ok(None);
                }
                let mut taken = allocator.document_collision_ids(&state.doc);
                let promotion = allocation::Promotion::prepare(state, allocator, &mut taken)?;
                let Some(page) = allocation::clone_page(state, index, name, allocator, &mut taken)?
                else {
                    return Ok(None);
                };
                Ok(Some(Kind::Insert {
                    promotion,
                    page,
                    at: index + 1,
                    legacy,
                    source: Some(index),
                }))
            }
            C::DeletePage { index } => {
                let index = index as usize;
                Ok(explicit
                    .filter(|pages| pages.len() > 1 && index < pages.len())
                    .map(|_| Kind::Delete { index }))
            }
            C::ReorderPage { from, to } => {
                let from = from as usize;
                Ok(explicit
                    .filter(|pages| from < pages.len())
                    .and_then(|pages| {
                        let to = (to as usize).min(pages.len() - 1);
                        (from != to).then_some(Kind::Move { from, to })
                    }))
            }
            _ => Ok(None),
        };
        result.map(|value| value.map(Self))
    }
    pub(crate) fn added(&self) -> Option<&PenPage> {
        match &self.0 {
            Kind::Insert { page, .. } => Some(page),
            _ => None,
        }
    }
    pub(crate) fn added_index(&self) -> Option<usize> {
        match &self.0 {
            Kind::Insert { at, .. } => Some(*at),
            _ => None,
        }
    }
    pub(crate) fn promoted_id(&self) -> Option<&str> {
        match &self.0 {
            Kind::Insert { promotion, .. } => promotion.id().map(|id| id.as_str()),
            _ => None,
        }
    }
    pub(crate) fn source<'a>(&self, state: &'a EditorState) -> Option<&'a [super::PenNode]> {
        match &self.0 {
            Kind::Insert {
                source: Some(index),
                ..
            } => match state.doc.pages.as_ref().filter(|pages| !pages.is_empty()) {
                Some(pages) => Some(pages.get(*index)?.children.as_slice()),
                None if *index == 0 => Some(state.doc.children.as_slice()),
                _ => None,
            },
            _ => None,
        }
    }
    pub(crate) fn removed<'a>(&self, state: &'a EditorState) -> Option<&'a PenPage> {
        match &self.0 {
            Kind::Delete { index } => state.doc.pages.as_ref()?.get(*index),
            _ => None,
        }
    }
    pub(crate) fn commit(self, state: &mut EditorState) -> CommittedPageCommand {
        match self.0 {
            Kind::Insert {
                promotion,
                page,
                at,
                legacy,
                ..
            } => {
                let id = page.id.clone();
                promotion.commit(state);
                state.doc.pages.as_mut().unwrap().insert(at, page);
                state.ui.active_page_index = at;
                state.clear_selection();
                CommittedPageCommand::Insert { id, at, legacy }
            }
            Kind::Delete { index } => {
                let page = state
                    .remove_page_retained(index)
                    .expect("fresh validated page removal");
                CommittedPageCommand::Delete { page, index }
            }
            Kind::Move { from, to } => {
                let id = state.doc.pages.as_ref().unwrap()[from].id.clone();
                assert!(state.reorder_page(from, to));
                CommittedPageCommand::Move { id, from }
            }
        }
    }
}
