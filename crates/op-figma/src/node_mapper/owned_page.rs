//! Ownership-only page source reduction; complete master subtrees survive.
use super::*;
use crate::progress::FigImportPhase;
use crate::tree::TreeNode;
fn masters(node: TreeNode, out: &mut Vec<TreeNode>, wanted: &std::collections::HashSet<String>) {
    if node.figma.get_str("type") == Some("SYMBOL") && {
        node.figma
            .get("guid")
            .and_then(crate::tree::guid_to_string)
            .is_some_and(|id| wanted.contains(&id))
    } {
        out.push(node)
    } else {
        for child in node.children {
            masters(child, out, wanted)
        }
    }
}
pub(crate) fn convert_tree_page_owned(
    tree: TreeNode,
    blobs: Vec<crate::figma_types::BlobOrString>,
    file_name: &str,
    page_index: usize,
    layout_mode: FigLayoutMode,
    observer: &mut dyn FnMut(FigImportPhase),
) -> FigmaImportResult {
    let mut symbol_index = HashMap::new();
    collect_symbol_tree(&tree, &mut symbol_index);
    fn symbol_count(n: &TreeNode) -> usize {
        usize::from(n.figma.get_str("type") == Some("SYMBOL") && n.figma.get("guid").is_some())
            + n.children.iter().map(symbol_count).sum::<usize>()
    }
    if tree.figma.get_str("type") == Some("SYMBOL") || symbol_count(&tree) != symbol_index.len() {
        let result = super::convert_tree_page(&tree, blobs, file_name, page_index, layout_mode);
        observer(FigImportPhase::ReleaseSource);
        drop(tree);
        return result;
    }
    let selected = tree
        .children
        .iter()
        .enumerate()
        .filter(|(_, p)| is_user_page(p))
        .nth(page_index)
        .or_else(|| {
            tree.children
                .iter()
                .enumerate()
                .find(|(_, p)| is_user_page(p))
        })
        .map(|(i, _)| i);
    let Some(selected) = selected else {
        observer(FigImportPhase::ReleaseSource);
        drop(tree);
        return empty_import_result(file_name, "No pages found in Figma file", blobs);
    };
    let mut component_map = HashMap::new();
    let mut counter = 1;
    collect_components(&tree.children[selected], &mut component_map, &mut counter);
    let mut assignments = HashMap::new();
    let wanted = super::dependencies::closure(&tree.children[selected], &symbol_index);
    crate::instance::seed_assignments_for_symbols(&tree, &symbol_index, &mut assignments, &wanted);
    drop(symbol_index);
    let mut selected_page = None;
    let mut pool = Vec::new();
    let TreeNode { figma, children } = tree;
    drop(figma);
    for (index, child) in children.into_iter().enumerate() {
        if index == selected {
            selected_page = Some(child)
        } else {
            masters(child, &mut pool, &wanted)
        }
    }
    let page = selected_page.unwrap();
    let mut symbols = HashMap::new();
    collect_symbol_tree(&page, &mut symbols);
    for master in &pool {
        collect_symbol_tree(master, &mut symbols)
    }
    let mut ctx = ConversionContext {
        component_map,
        symbol_tree: symbols,
        warnings: Vec::new(),
        id_counter: counter,
        blobs,
        layout_mode,
        instance_assignments: assignments,
        instance_expansions: Default::default(),
    };
    let children = convert_children(&page, &mut ctx);
    let name = page
        .figma
        .get_str("name")
        .map(str::to_owned)
        .unwrap_or_else(|| "Page 1".into());
    let pen = pen_page(&page, format!("figma-page-{page_index}"), name, children);
    let document = document_with_pages(file_name, vec![pen]);
    let blobs = std::mem::take(&mut ctx.blobs);
    let warnings = std::mem::take(&mut ctx.warnings);
    drop(ctx);
    observer(FigImportPhase::ReleaseSource);
    drop(page);
    drop(pool);
    let image_blobs = collect_referenced_image_blobs(&document, blobs);
    FigmaImportResult {
        document,
        warnings,
        image_blobs,
    }
}

#[cfg(test)]
#[path = "owned_page_tests.rs"]
mod tests;
