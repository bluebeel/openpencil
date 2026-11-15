//! Validated immutable top-level node frames and their compact metadata.
use super::{deferred::Shared, DecodeScope, FigValue, ResolvedType};
use std::sync::Arc;

pub(super) struct Frame {
    pub bytes: Box<[u8]>,
    pub kind: ResolvedType,
    pub trailing: usize,
}

pub(crate) struct EncodedNodes {
    frames: Vec<Option<Frame>>,
    shared: Arc<Shared>,
}
impl std::fmt::Debug for EncodedNodes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncodedNodes")
            .field("nodes", &self.frames.len())
            .finish()
    }
}
impl EncodedNodes {
    pub(super) fn new(frames: Vec<Frame>, shared: Arc<Shared>) -> Self {
        Self {
            frames: frames.into_iter().map(Some).collect(),
            shared,
        }
    }
    fn hydrate(&mut self, index: usize, value: &mut FigValue) {
        if let Some(frame) = self.frames[index].take() {
            // Release the metadata allocation before replacing it with the
            // complete record. No borrowed view points into frame storage.
            *value = FigValue::Null;
            *value = self.shared.decode_value(
                &frame.bytes,
                frame.kind,
                1,
                DecodeScope::DirectNode,
                frame.trailing,
            );
        }
    }
    pub(crate) fn all(mut self, mut nodes: Vec<FigValue>) -> Vec<FigValue> {
        assert_eq!(nodes.len(), self.frames.len(), "validated node frame count");
        for (i, node) in nodes.iter_mut().enumerate() {
            self.hydrate(i, node);
        }
        nodes
    }
    pub(crate) fn page(
        mut self,
        nodes: Vec<FigValue>,
        topology: &crate::node_mapper::source_selection::Topology,
        page: usize,
    ) -> Vec<FigValue> {
        assert_eq!(nodes.len(), self.frames.len(), "validated node frame count");
        crate::node_mapper::source_selection::select(nodes, topology, page, |i, node| {
            self.hydrate(i, node)
        })
    }
}
