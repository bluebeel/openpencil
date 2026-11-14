//! Prepare only page identifiers and intended clone payloads. Nothing live is
//! moved until every fallible allocation has completed.
use super::{make_page, walkers, EditorState, IdAllocError, IdAllocator, NodeId, PenNode, PenPage};
use std::collections::HashSet;
pub(super) struct Promotion(Option<NodeId>);
impl Promotion {
    pub(super) fn prepare(
        state: &EditorState,
        allocator: &mut dyn IdAllocator,
        taken: &mut HashSet<NodeId>,
    ) -> Result<Self, IdAllocError> {
        let needs = state
            .doc
            .pages
            .as_ref()
            .is_none_or(|pages| pages.is_empty());
        Ok(Self(if needs {
            Some(allocator.allocate(taken)?)
        } else {
            None
        }))
    }
    pub(super) fn commit(self, state: &mut EditorState) {
        if let Some(id) = self.0 {
            let root = std::mem::take(&mut state.doc.children);
            state.doc.pages = Some(vec![make_page(id.into(), "Page 1".to_owned(), root)]);
        }
    }
}
pub(super) fn clone_page(
    state: &EditorState,
    index: usize,
    name: Option<String>,
    allocator: &mut dyn IdAllocator,
    taken: &mut HashSet<NodeId>,
) -> Result<Option<PenPage>, IdAllocError> {
    let explicit = state.doc.pages.as_ref().filter(|pages| !pages.is_empty());
    let source = match explicit {
        Some(pages) => match pages.get(index) {
            Some(page) => Some(page),
            None => return Ok(None),
        },
        None => {
            if index != 0 {
                return Ok(None);
            }
            None
        }
    };
    let id = allocator.allocate(taken)?;
    let children = source.map_or(state.doc.children.as_slice(), |page| {
        page.children.as_slice()
    });
    let fresh: Result<Vec<PenNode>, _> = children
        .iter()
        .map(|node| walkers::deep_clone_with_allocator(node, allocator, taken))
        .collect();
    let children = fresh?;
    let name = name.unwrap_or_else(|| {
        format!(
            "{} copy",
            source.map_or("Page 1", |page| page.name.as_str())
        )
    });
    Ok(Some(match source {
        Some(page) => PenPage {
            id: id.into(),
            name,
            children,
            background_color: page.background_color.clone(),
            state: page.state.clone(),
            lifecycle: page.lifecycle.clone(),
        },
        None => make_page(id.into(), name, children),
    }))
}
