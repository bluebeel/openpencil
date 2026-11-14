//! Owned import discovery over exact validated node frames.
use super::*;
use crate::kiwi::indexed::EncodedNodes;

pub(crate) struct IndexedFile {
    pub decoded: FigmaDecodedFile,
    pub encoded: Option<EncodedNodes>,
}

pub(crate) fn parse_indexed_owned(
    bytes: Vec<u8>,
    observer: &mut dyn FnMut(FigImportPhase),
) -> Result<IndexedFile, FigFileError> {
    observer(FigImportPhase::Container);
    let container = fig_to_encoded_parts(&bytes).map_err(FigFileError::Container)?;
    drop(bytes);
    if container.parts.len() < 2 {
        return Err(FigFileError::TooFewParts(container.parts.len()));
    }
    observer(FigImportPhase::Schema);
    let chunk = crate::container::stream::chunk(&container.parts[0]);
    let mut bytes = Vec::with_capacity(chunk.len);
    std::io::Read::read_to_end(&mut chunk.reader(), &mut bytes)
        .map_err(|_| FigFileError::Kiwi(KiwiError::OutOfBounds))?;
    let schema = decode_binary_schema(&bytes).map_err(FigFileError::Kiwi)?;
    // Schema owns its strings/tables; its inflated wire buffer is no longer
    // needed while the much larger node stream is indexed.
    drop(bytes);
    observer(FigImportPhase::Decode);
    let chunk = crate::container::stream::chunk(&container.parts[1]);
    let indexed = crate::kiwi::stream::decode_indexed(
        &schema,
        std::io::BufReader::with_capacity(64 * 1024, chunk.reader()),
        chunk.len,
    )
    .map_err(FigFileError::Kiwi)?;
    observer(FigImportPhase::Extract);
    drop(container.parts);
    let (node_changes, mut blobs) =
        extract_decoded_root(indexed.root).ok_or(FigFileError::NoNodeChanges)?;
    if let Some(raw) = indexed.blobs {
        blobs = raw;
    }
    Ok(IndexedFile {
        decoded: FigmaDecodedFile {
            node_changes,
            blobs,
            image_files: container.image_files,
        },
        encoded: indexed.nodes,
    })
}
