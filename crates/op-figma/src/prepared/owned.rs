//! Lazy owned discovery; page choice precedes complete source materialization.
use super::*;
use crate::figma_types::{indexed::IndexedFile, FigmaDecodedFile};
use crate::kiwi::{indexed::EncodedNodes, FigValue};
use crate::node_mapper::source_selection::Topology;

pub(super) struct IndexedPreparation {
    metadata: Vec<FigValue>,
    encoded: EncodedNodes,
    topology: Topology,
}
impl std::fmt::Debug for IndexedPreparation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IndexedPreparation")
            .field("nodes", &self.metadata.len())
            .finish()
    }
}
impl IndexedPreparation {
    pub(super) fn into_tree(
        self,
        page: Option<usize>,
        layout: FigLayoutMode,
        observer: &mut dyn FnMut(FigImportPhase),
    ) -> Option<TreeNode> {
        let Self {
            metadata,
            encoded,
            topology,
        } = self;
        let nodes = match page {
            Some(page) => {
                let nodes = encoded.page(metadata, &topology, page);
                drop(topology);
                nodes
            }
            None => {
                // Discovery ancestry is no longer read by full hydration or
                // style/tree construction. Release its indexes before either.
                drop(topology);
                encoded.all(metadata)
            }
        };
        resolve_tree(nodes, layout, observer)
    }
}

pub(super) fn prepare(
    bytes: Vec<u8>,
    file_name: &str,
    layout: FigLayoutMode,
    observer: &mut dyn FnMut(FigImportPhase),
) -> Result<PreparedFig, FigParseError> {
    let IndexedFile { decoded, encoded } =
        crate::figma_types::indexed::parse_indexed_owned(bytes, observer)
            .map_err(|error| FigParseError::Binary(error.to_string()))?;
    let FigmaDecodedFile {
        node_changes,
        blobs,
        image_files,
    } = decoded;
    match (encoded, Topology::new(&node_changes)) {
        (Some(encoded), Some(topology)) => {
            let pages = topology.pages(&node_changes);
            let prepared = PreparedFig {
                file_name: file_name.into(),
                layout_mode: layout,
                tree: None,
                indexed: Some(IndexedPreparation {
                    metadata: node_changes,
                    encoded,
                    topology,
                }),
                pages,
                blobs,
                image_files,
            };
            observer(FigImportPhase::Prepared);
            Ok(prepared)
        }
        (encoded, _) => {
            // Unknown root layouts and ambiguous/cyclic ancestry use every
            // complete historical record, with the existing blob packing.
            let blobs = blobs.into_iter().map(BlobOrString::packed).collect();
            let node_changes = match encoded {
                Some(encoded) => encoded.all(node_changes),
                None => node_changes,
            };
            Ok(PreparedFig::from_decoded_with_observer(
                FigmaDecodedFile {
                    node_changes,
                    blobs,
                    image_files,
                },
                file_name,
                layout,
                observer,
            ))
        }
    }
}

pub(super) fn resolve_tree(
    mut nodes: Vec<FigValue>,
    layout: FigLayoutMode,
    observer: &mut dyn FnMut(FigImportPhase),
) -> Option<TreeNode> {
    observer(FigImportPhase::ResolveStyles);
    resolve_style_references_with_mode(&mut nodes, layout);
    observer(FigImportPhase::BuildTree);
    build_tree_owned(nodes)
}
