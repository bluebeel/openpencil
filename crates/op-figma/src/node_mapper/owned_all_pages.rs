//! Full-document ownership handoff; only component-bearing roots stay pinned.
//! Canonical order, global component IDs and pooled instance assignments are
//! established against the complete source before any root leaves memory.
use super::*;

// Moving a root into this pool preserves the complete master subtree and its
// unknown fields. No component/source node is cloned merely to shorten a borrow.
enum Root {
    Owned(TreeNode),
    Retained(usize),
}
struct Page {
    shell: TreeNode,
    roots: Vec<Root>,
}
fn contains_symbol(node: &TreeNode) -> bool {
    node.figma.get_str("type") == Some("SYMBOL") || node.children.iter().any(contains_symbol)
}
fn retain(root: TreeNode, pool: &mut Vec<TreeNode>) -> Root {
    if contains_symbol(&root) {
        let index = pool.len();
        pool.push(root);
        Root::Retained(index)
    } else {
        Root::Owned(root)
    }
}

pub(crate) fn convert_tree_all_pages_owned(
    tree: TreeNode,
    blobs: Vec<crate::figma_types::BlobOrString>,
    file_name: &str,
    layout_mode: FigLayoutMode,
) -> FigmaImportResult {
    // Historical non-document roots can themselves be referenced components.
    // Keep their original borrowed conversion rather than changing resolution.
    if tree.figma.get_str("type") == Some("SYMBOL") {
        return convert_tree_all_pages(&tree, blobs, file_name, layout_mode);
    }
    let mut component_map = HashMap::new();
    let mut counter = 1;
    let mut page_count = 0;
    for page in tree.children.iter().filter(|page| is_user_page(page)) {
        collect_components(page, &mut component_map, &mut counter);
        page_count += 1;
    }
    if page_count == 0 {
        return empty_import_result(file_name, "No pages found in Figma file", blobs);
    }
    let mut symbols = HashMap::new();
    collect_symbol_tree(&tree, &mut symbols);
    let mut assignments = HashMap::new();
    crate::instance::seed_assignments_from_instances(&tree, &symbols, &mut assignments);
    drop(symbols);

    let mut pool = Vec::new();
    let mut pages = Vec::with_capacity(page_count);
    let TreeNode { figma, children } = tree;
    drop(figma);
    // Append retained roots in original document DFS order. Rebuilding the
    // symbol lookup below therefore preserves duplicate-GUID precedence too.
    for page in children {
        if is_user_page(&page) {
            let TreeNode { figma, children } = page;
            let roots = children
                .into_iter()
                .map(|root| retain(root, &mut pool))
                .collect();
            pages.push(Page {
                shell: TreeNode {
                    figma,
                    children: Vec::new(),
                },
                roots,
            });
        } else if contains_symbol(&page) {
            pool.push(page);
        }
        // Non-user, non-master branches were only relevant to pooled seeding,
        // already completed above. The historical converter never emits them.
    }
    let mut symbols = HashMap::new();
    for root in &pool {
        collect_symbol_tree(root, &mut symbols);
    }
    let mut ctx = ConversionContext {
        component_map,
        symbol_tree: symbols,
        warnings: Vec::new(),
        id_counter: counter,
        retained_ids: Default::default(),
        blobs,
        layout_mode,
        instance_assignments: assignments,
        instance_expansions: Default::default(),
    };
    let mut pen_pages = Vec::with_capacity(page_count);
    for (index, Page { shell, roots }) in pages.into_iter().enumerate() {
        let stack = shell.figma.get_str("stackMode");
        let mut children = Vec::new();
        for root in roots {
            let node = match &root {
                Root::Owned(node) => node,
                Root::Retained(index) => &pool[*index],
            };
            if let Some(node) = crate::converters::convert_retained_child(node, stack, &mut ctx) {
                children.push(node);
            }
            // An owned source root leaves here, before the following root's
            // canonical children are allocated. Retained masters remain shared.
            drop(root);
        }
        let name = shell
            .figma
            .get_str("name")
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Page {}", index + 1));
        pen_pages.push(pen_page(
            &shell,
            format!("figma-page-{index}"),
            name,
            children,
        ));
    }
    let document = document_with_pages(file_name, pen_pages);
    let blobs = std::mem::take(&mut ctx.blobs);
    let warnings = std::mem::take(&mut ctx.warnings);
    drop(ctx);
    drop(pool);
    let image_blobs = collect_referenced_image_blobs(&document, blobs);
    FigmaImportResult {
        document,
        warnings,
        image_blobs,
    }
}
