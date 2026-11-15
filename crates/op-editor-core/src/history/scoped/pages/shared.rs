//! Complete immutable page payloads. Child trees are recursive SharedNodes;
//! metadata cloning never clones the page forest first.
use crate::{history_snapshot::command_capture_value_bytes, SharedNode};
use jian_ops_schema::page::PenPage;
use std::{mem, sync::Arc};

pub(super) fn header(page: &PenPage) -> PenPage {
    // Exhaustive schema match: a new page field cannot silently disappear.
    let PenPage {
        id,
        name,
        children: _,
        background_color,
        state,
        lifecycle,
    } = page;
    PenPage {
        id: id.clone(),
        name: name.clone(),
        children: Vec::new(),
        background_color: background_color.clone(),
        state: state.clone(),
        lifecycle: lifecycle.clone(),
    }
}
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SharedPage {
    pub(super) metadata: PenPage,
    pub(super) children: Arc<Vec<Arc<SharedNode>>>,
}
impl SharedPage {
    pub(super) fn owned(mut page: PenPage) -> Arc<Self> {
        let children = mem::take(&mut page.children)
            .into_iter()
            .map(SharedNode::capture_owned)
            .collect();
        Arc::new(Self {
            metadata: page,
            children: Arc::new(children),
        })
    }
    pub(super) fn id(&self) -> &str {
        &self.metadata.id
    }
    pub(super) fn materialize(&self) -> PenPage {
        let mut page = self.metadata.clone();
        page.children = self
            .children
            .iter()
            .map(|node| node.materialize())
            .collect();
        page
    }
    pub(super) fn bytes(&self) -> usize {
        mem::size_of::<Self>()
            + mem::size_of::<usize>() * 4
            + self.children.capacity() * mem::size_of::<Arc<SharedNode>>()
            + command_capture_value_bytes(&self.metadata, usize::MAX / 4).unwrap_or(usize::MAX / 4)
    }
}
