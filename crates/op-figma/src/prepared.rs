//! Two-stage binary Figma import. Borrowed preparation resolves the source
//! eagerly. Owned preparation validates and indexes exact node frames, then
//! expands source data after page selection. Both expose the same page metadata
//! and consume their source into one page or the complete document.

use crate::common::FigLayoutMode;
use crate::figma_types::{
    parse_fig_file, parse_fig_file_with_observer, BlobOrString, FigmaDecodedFile,
};
use crate::image_resolver::{resolve_image_blobs_owned_with, ImageTransform};
use crate::node_mapper::{
    convert_tree_all_pages, empty_import_result, resolve_style_references_with_mode,
    FigmaImportResult, FigmaPageInfo,
};
use crate::progress::FigImportPhase;
use crate::tree::{build_tree_owned, guid_to_string, is_user_page, TreeNode};
use crate::{detect_kind, FigFileKind, FigImport, FigParseError};
use std::collections::HashMap;

mod owned;

/// A decoded and indexed `.fig` file awaiting page conversion.
///
/// [`pages`](Self::pages) returns a borrowed metadata slice; it does
/// not rebuild or clone the document tree. Conversion consumes `self`
/// because the geometry and image blob pools are moved into the result.
#[derive(Debug)]
pub struct PreparedFig {
    file_name: String,
    layout_mode: FigLayoutMode,
    tree: Option<TreeNode>,
    indexed: Option<owned::IndexedPreparation>,
    pages: Vec<FigmaPageInfo>,
    blobs: Vec<BlobOrString>,
    image_files: HashMap<String, Vec<u8>>,
}

/// Decode and prepare a binary `.fig` for page discovery and deferred
/// conversion.
pub fn prepare_fig_binary(
    bytes: &[u8],
    file_name: &str,
    layout_mode: FigLayoutMode,
) -> Result<PreparedFig, FigParseError> {
    if detect_kind(bytes) != FigFileKind::Binary {
        return Err(FigParseError::UnknownFormat);
    }
    let decoded = parse_fig_file(bytes).map_err(|e| FigParseError::Binary(e.to_string()))?;
    Ok(PreparedFig::from_decoded(decoded, file_name, layout_mode))
}

/// Preparation with typed phase notifications; the ordinary API remains silent.
pub fn prepare_fig_binary_with_observer(
    bytes: &[u8],
    file_name: &str,
    layout_mode: FigLayoutMode,
    observer: &mut dyn FnMut(FigImportPhase),
) -> Result<PreparedFig, FigParseError> {
    if detect_kind(bytes) != FigFileKind::Binary {
        return Err(FigParseError::UnknownFormat);
    }
    let decoded = parse_fig_file_with_observer(bytes, observer)
        .map_err(|error| FigParseError::Binary(error.to_string()))?;
    Ok(PreparedFig::from_decoded_with_observer(
        decoded,
        file_name,
        layout_mode,
        observer,
    ))
}

/// Consume the compressed source and defer complete node expansion until conversion.
/// The borrowed preparation API remains available for reusable input buffers.
pub fn prepare_fig_binary_owned(
    bytes: Vec<u8>,
    file_name: &str,
    layout_mode: FigLayoutMode,
) -> Result<PreparedFig, FigParseError> {
    prepare_fig_binary_owned_with_observer(bytes, file_name, layout_mode, &mut |_| {})
}

/// Owned preparation with the same typed phase notifications as the borrowed API.
pub fn prepare_fig_binary_owned_with_observer(
    bytes: Vec<u8>,
    file_name: &str,
    layout_mode: FigLayoutMode,
    observer: &mut dyn FnMut(FigImportPhase),
) -> Result<PreparedFig, FigParseError> {
    if detect_kind(&bytes) != FigFileKind::Binary {
        return Err(FigParseError::UnknownFormat);
    }
    owned::prepare(bytes, file_name, layout_mode, observer)
}

impl PreparedFig {
    fn from_decoded(
        decoded: FigmaDecodedFile,
        file_name: &str,
        layout_mode: FigLayoutMode,
    ) -> Self {
        Self::from_decoded_with_observer(decoded, file_name, layout_mode, &mut |_| {})
    }

    fn from_decoded_with_observer(
        mut decoded: FigmaDecodedFile,
        file_name: &str,
        layout_mode: FigLayoutMode,
        observer: &mut dyn FnMut(FigImportPhase),
    ) -> Self {
        let tree = owned::resolve_tree(
            std::mem::take(&mut decoded.node_changes),
            layout_mode,
            observer,
        );
        let pages = tree
            .as_ref()
            .map(|tree| {
                tree.children
                    .iter()
                    .filter(|child| is_user_page(child))
                    .map(page_info)
                    .collect()
            })
            .unwrap_or_default();
        let prepared = Self {
            file_name: file_name.to_string(),
            layout_mode,
            tree,
            indexed: None,
            pages,
            blobs: decoded.blobs,
            image_files: decoded.image_files,
        };
        observer(FigImportPhase::Prepared);
        prepared
    }

    /// User-visible pages in Figma page-panel order.
    pub fn pages(&self) -> &[FigmaPageInfo] {
        &self.pages
    }

    /// Convert one page without applying a host image transform.
    pub fn into_page(self, page_index: usize) -> Result<FigImport, FigParseError> {
        self.into_page_with_images(page_index, None)
    }

    /// Convert one page with notifications around conversion, source release
    /// and image resolution. Existing conversion APIs retain a no-op observer.
    pub fn into_page_with_observer(
        self,
        page_index: usize,
        observer: &mut dyn FnMut(FigImportPhase),
    ) -> Result<FigImport, FigParseError> {
        self.into_page_with_images_and_observer(page_index, None, observer)
    }

    /// Convert one page, transforming each referenced image at most
    /// once before embedding it as a data URL.
    pub fn into_page_with_images(
        self,
        page_index: usize,
        image_transform: Option<&ImageTransform<'_>>,
    ) -> Result<FigImport, FigParseError> {
        self.into_page_with_images_and_observer(page_index, image_transform, &mut |_| {})
    }

    fn into_page_with_images_and_observer(
        self,
        page_index: usize,
        image_transform: Option<&ImageTransform<'_>>,
        observer: &mut dyn FnMut(FigImportPhase),
    ) -> Result<FigImport, FigParseError> {
        let page_count = self.pages.len();
        if page_index >= page_count {
            return Err(FigParseError::PageOutOfBounds {
                index: page_index,
                page_count,
            });
        }
        let Self {
            file_name,
            layout_mode,
            tree,
            indexed,
            blobs,
            image_files,
            ..
        } = self;
        let tree = match indexed {
            Some(source) => source.into_tree(Some(page_index), layout_mode, observer),
            None => tree,
        };
        let tree = tree.expect("prepared file with pages must have a document tree");
        observer(FigImportPhase::ConvertPage);
        let result = crate::node_mapper::convert_tree_page_owned(
            tree,
            blobs,
            &file_name,
            page_index,
            layout_mode,
            observer,
        );
        observer(FigImportPhase::ResolveImages);
        let result = resolve_images(result, image_files, image_transform);
        observer(FigImportPhase::Complete);
        Ok(result)
    }

    /// Convert every user-visible page without a host image transform.
    pub fn into_all_pages(self) -> Result<FigImport, FigParseError> {
        self.into_all_pages_with_images(None)
    }

    /// Convert every user-visible page, transforming each referenced
    /// image at most once before embedding it as a data URL.
    pub fn into_all_pages_with_images(
        self,
        image_transform: Option<&ImageTransform<'_>>,
    ) -> Result<FigImport, FigParseError> {
        let Self {
            file_name,
            layout_mode,
            tree,
            indexed,
            mut blobs,
            image_files,
            ..
        } = self;
        let tree = match indexed {
            Some(source) => {
                blobs = blobs.into_iter().map(BlobOrString::packed).collect();
                source.into_tree(None, layout_mode, &mut |_| {})
            }
            None => tree,
        };
        let result = match tree {
            Some(tree) => {
                let result = convert_tree_all_pages(&tree, blobs, &file_name, layout_mode);
                drop(tree);
                result
            }
            None => empty_import_result(&file_name, "No document root found", blobs),
        };
        Ok(resolve_images(result, image_files, image_transform))
    }
}

fn page_info(page: &TreeNode) -> FigmaPageInfo {
    FigmaPageInfo {
        id: page
            .figma
            .get("guid")
            .and_then(guid_to_string)
            .unwrap_or_default(),
        name: page.figma.get_str("name").unwrap_or("Page").to_string(),
        child_count: page.children.len(),
    }
}

fn resolve_images(
    result: FigmaImportResult,
    image_files: HashMap<String, Vec<u8>>,
    image_transform: Option<&ImageTransform<'_>>,
) -> FigImport {
    let FigmaImportResult {
        mut document,
        warnings,
        image_blobs,
    } = result;
    resolve_image_blobs_owned_with(&mut document, image_blobs, image_files, image_transform);
    FigImport { document, warnings }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kiwi::FigValue;

    fn object(fields: Vec<(&str, FigValue)>) -> FigValue {
        FigValue::Object(
            fields
                .into_iter()
                .map(|(name, value)| (name.into(), value))
                .collect(),
        )
    }

    fn guid(local_id: u32) -> FigValue {
        object(vec![
            ("sessionID", FigValue::Uint(0)),
            ("localID", FigValue::Uint(local_id)),
        ])
    }

    fn node(local_id: u32, node_type: &str, name: &str, parent: Option<u32>) -> FigValue {
        let mut fields = vec![
            ("guid", guid(local_id)),
            ("type", FigValue::Str(node_type.to_string().into())),
            ("name", FigValue::Str(name.to_string().into())),
        ];
        if let Some(parent) = parent {
            fields.push((
                "parentIndex",
                object(vec![
                    ("guid", guid(parent)),
                    ("position", FigValue::Str(format!("{local_id:04}").into())),
                ]),
            ));
        }
        object(fields)
    }

    fn decoded() -> FigmaDecodedFile {
        FigmaDecodedFile {
            node_changes: vec![
                node(1, "DOCUMENT", "Doc", None),
                node(2, "CANVAS", "First", Some(1)),
                node(3, "RECTANGLE", "Box", Some(2)),
                node(4, "CANVAS", "Internal Only Assets", Some(1)),
                node(5, "CANVAS", "Second", Some(1)),
            ],
            blobs: Vec::new(),
            image_files: HashMap::new(),
        }
    }

    #[test]
    fn pages_are_borrowed_metadata_in_user_page_order() {
        let prepared = PreparedFig::from_decoded(decoded(), "Test", FigLayoutMode::Preserve);
        assert_eq!(prepared.pages().len(), 2);
        assert_eq!(prepared.pages()[0].name, "First");
        assert_eq!(prepared.pages()[0].child_count, 1);
        assert_eq!(prepared.pages()[1].name, "Second");
    }

    #[test]
    fn into_page_is_strict_and_converts_only_the_selected_page() {
        let prepared = PreparedFig::from_decoded(decoded(), "Test", FigLayoutMode::Preserve);
        let import = prepared.into_page(1).expect("second page converts");
        let pages = import.document.pages.expect("document pages");
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].id, "figma-page-1");
        assert_eq!(pages[0].name, "Second");
    }

    #[test]
    fn into_page_reports_requested_index_and_page_count() {
        let prepared = PreparedFig::from_decoded(decoded(), "Test", FigLayoutMode::Preserve);
        let error = prepared.into_page(2).expect_err("index 2 is out of range");
        assert!(matches!(
            error,
            FigParseError::PageOutOfBounds {
                index: 2,
                page_count: 2
            }
        ));
    }
}
