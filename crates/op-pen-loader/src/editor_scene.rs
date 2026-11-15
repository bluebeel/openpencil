//! Editor-state scene builders.
//!
//! The full builder remains available for export, MCP, and preview consumers.
//! Interactive hosts use the active-page builder so inactive pages do not pay
//! ref/token expansion, layout, payload, or scene-allocation costs.

use jian_scene::layout_scene::{LayoutScene, ScenePage};
use op_editor_core::scene_vars::VariableTable;

use crate::editor_state_var_table;
use crate::payload::DocPayload;

/// Build a paint-only [`LayoutScene`] from every page in an editor state.
pub fn editor_state_to_layout_scene(state: &op_editor_core::EditorState) -> LayoutScene {
    full_scene(
        state,
        state.editor_ui.workspace.visible,
        normal_presentation(state),
    )
}
pub fn editor_state_to_export_layout_scene(state: &op_editor_core::EditorState) -> LayoutScene {
    full_scene(state, true, state.editor_ui.workspace.visible)
}
/// Normal mode is a reading surface; Preview keeps its authored device viewport.
pub(crate) fn normal_presentation(state: &op_editor_core::EditorState) -> bool {
    state.editor_ui.workspace.visible && !state.editor_ui.preview.mode
}

fn full_scene(state: &op_editor_core::EditorState, paged: bool, full_content: bool) -> LayoutScene {
    let mut prepared = std::borrow::Cow::Borrowed(&state.doc);
    if op_editor_core::ref_resolve::document_has_refs(&prepared) {
        prepared = std::borrow::Cow::Owned(op_editor_core::ref_resolve::resolve_refs_for_canvas(
            &prepared,
        ));
    }
    if op_editor_core::variables_resolve::document_has_tokens(&prepared) {
        prepared = std::borrow::Cow::Owned(
            op_editor_core::variables_resolve::resolve_document_for_canvas(
                &prepared,
                &state.ui.variables.active_theme,
            ),
        );
    }
    let root_snapshot = crate::table_default_snapshot::roots(&prepared.children, &state.doc, paged);
    let page_snapshots: Vec<_> = prepared
        .pages
        .as_ref()
        .into_iter()
        .flatten()
        .map(|page| crate::table_default_snapshot::roots(&page.children, &state.doc, paged))
        .collect();
    if root_snapshot.is_some() || page_snapshots.iter().any(Option::is_some) {
        let mut snapshot = prepared.into_owned();
        if let Some(nodes) = root_snapshot {
            snapshot.children = nodes;
        }
        if let Some(pages) = &mut snapshot.pages {
            for (page, nodes) in pages.iter_mut().zip(page_snapshots) {
                if let Some(nodes) = nodes {
                    page.children = nodes;
                }
            }
        }
        prepared = std::borrow::Cow::Owned(snapshot);
    }
    if full_content
        && !state.editor_ui.preserve_authored_geometry
        && document_needs_presentation(&prepared)
    {
        let mut snapshot = prepared.into_owned();
        let original = snapshot.clone();
        prepare_presentation(&mut snapshot.children, &original, None);
        if let Some(pages) = &mut snapshot.pages {
            for page in pages {
                prepare_presentation(&mut page.children, &original, Some(&page.id));
            }
        }
        prepared = std::borrow::Cow::Owned(snapshot);
    }
    let payload: DocPayload = if state.editor_ui.preserve_authored_geometry {
        crate::adapter::pen_document_to_payload_preserving_geometry(&prepared).payload
    } else {
        crate::adapter::pen_document_to_payload(&prepared).payload
    };
    // Payload fields are owned/shared. Do not retain an expanded canonical
    // graph while allocating the second full paint representation below.
    drop(prepared);
    let var_table: VariableTable = editor_state_var_table(state);

    crate::layout_scene::payload_into_scene(payload, &var_table, state.ui.active_page_index)
}

/// Build the interactive editor scene for only the active page.
///
/// Page metadata stays in document order and `active_page_index` keeps its
/// canonical index, but inactive [`ScenePage`] entries have no render children.
pub fn editor_state_to_active_page_layout_scene(
    state: &op_editor_core::EditorState,
) -> LayoutScene {
    active_scene(
        state,
        state.editor_ui.workspace.visible,
        normal_presentation(state),
    )
}
pub fn editor_state_to_active_page_export_layout_scene(
    state: &op_editor_core::EditorState,
) -> LayoutScene {
    active_export_scene(state, state.editor_ui.workspace.visible)
}

/// Build a static, full-content presentation of vertical screens and scroll areas.
///
/// Only an ephemeral export snapshot grows. The editable document, ordinary
/// professional canvas, and Preview retain their authored viewport and fixed chrome.
pub fn editor_state_to_active_page_full_content_export_layout_scene(
    state: &op_editor_core::EditorState,
) -> LayoutScene {
    active_export_scene(state, true)
}

fn active_export_scene(state: &op_editor_core::EditorState, full_content: bool) -> LayoutScene {
    let scene = active_scene(state, true, full_content);
    if state.selection_count() == 1
        && state.selection.anchor.is_real()
        && scene
            .active_page()
            .is_some_and(|page| page.find(state.selection.anchor.as_str()).is_none())
    {
        // A specifically selected record remains exportable outside page one.
        active_scene(state, false, full_content)
    } else {
        scene
    }
}
fn active_scene(
    state: &op_editor_core::EditorState,
    paged: bool,
    full_content: bool,
) -> LayoutScene {
    let (mut pages, active_page_index, roots): (
        Vec<ScenePage>,
        usize,
        &[jian_ops_schema::node::PenNode],
    ) = if let Some(doc_pages) = state.doc.pages.as_ref() {
        if doc_pages.is_empty() {
            return LayoutScene::default();
        }
        let active_page_index = state
            .ui
            .active_page_index
            .min(doc_pages.len().saturating_sub(1));
        let pages = doc_pages
            .iter()
            .map(|page| ScenePage {
                id: page.id.clone(),
                name: page.name.clone(),
                children: Vec::new(),
            })
            .collect();
        (
            pages,
            active_page_index,
            &doc_pages[active_page_index].children,
        )
    } else {
        let (id, name) = if state.doc.children.is_empty() {
            ("n1".to_string(), "Page 1".to_string())
        } else {
            (
                "page-1".to_string(),
                state.doc.name.as_deref().unwrap_or("Page 1").to_string(),
            )
        };
        (
            vec![ScenePage {
                id,
                name,
                children: Vec::new(),
            }],
            0,
            state.doc.children.as_slice(),
        )
    };

    pages[active_page_index].children = roots_to_scene_children(
        state,
        roots,
        &pages[active_page_index],
        active_page_index,
        paged,
        full_content,
    );

    LayoutScene {
        pages,
        active_page_index,
    }
}

/// Resolve and lay out complete top-level roots against the original document.
/// Unrelated roots may be omitted, but ancestors and flex siblings may not.
pub(crate) fn roots_to_scene_children(
    state: &op_editor_core::EditorState,
    roots: &[jian_ops_schema::node::PenNode],
    page: &ScenePage,
    page_index: usize,
    paged: bool,
    full_content: bool,
) -> Vec<jian_scene::layout_scene::SceneNode> {
    let payload = roots_to_page_payload(state, roots, page, page_index, paged, full_content);
    let var_table = editor_state_var_table(state);
    payload
        .children
        .into_iter()
        .map(|node| crate::layout_scene::node_payload_to_scene(node, &var_table, 1.0))
        .collect()
}

pub(crate) fn roots_to_page_payload(
    state: &op_editor_core::EditorState,
    roots: &[jian_ops_schema::node::PenNode],
    page: &ScenePage,
    page_index: usize,
    paged: bool,
    full_content: bool,
) -> crate::payload::PagePayload {
    let prepared = prepared_page_roots(state, roots, page, paged, full_content);
    let payload = crate::adapter::pen_roots_to_page_payload(
        &page.id,
        &page.name,
        prepared.as_ref(),
        page_index,
        state.editor_ui.preserve_authored_geometry,
    );
    // Every payload field is owned/shared; release expanded canonical roots
    // before a second representation is allocated by the caller.
    drop(prepared);
    payload
}

/// Shared exact ref/token/table/presentation preparation for bounded geometry.
pub(crate) fn prepared_page_roots<'a>(
    state: &op_editor_core::EditorState,
    roots: &'a [jian_ops_schema::node::PenNode],
    page: &ScenePage,
    paged: bool,
    full_content: bool,
) -> std::borrow::Cow<'a, [jian_ops_schema::node::PenNode]> {
    let page_id = state.doc.pages.as_ref().map(|_| page.id.as_str());
    let mut prepared = std::borrow::Cow::Borrowed(roots);
    if op_editor_core::ref_resolve::roots_have_refs(prepared.as_ref()) {
        prepared = std::borrow::Cow::Owned(if state.document_revision() == 0 {
            // A freshly loaded/imported state's registry was built from this
            // exact document. Reusing it avoids indexing every inactive page
            // on each switch. Once any canonical edit has happened the
            // registry may be a stale prototype snapshot, so preserve the old
            // live-document semantics by taking the full lookup path instead.
            op_editor_core::ref_resolve::resolve_refs_for_canvas_roots_with_components(
                prepared.as_ref(),
                &state.components,
                &state.doc,
            )
        } else {
            op_editor_core::ref_resolve::resolve_refs_for_canvas_roots(
                prepared.as_ref(),
                &state.doc,
            )
        });
    }
    if op_editor_core::variables_resolve::roots_have_tokens(prepared.as_ref()) {
        let mut owned = prepared.into_owned();
        op_editor_core::variables_resolve::resolve_roots_for_canvas(
            &mut owned,
            &state.doc,
            &state.ui.variables.active_theme,
        );
        prepared = std::borrow::Cow::Owned(owned);
    }

    if let Some(nodes) = crate::table_default_snapshot::roots(prepared.as_ref(), &state.doc, paged)
    {
        prepared = std::borrow::Cow::Owned(nodes);
    }
    prepared = roots_with_presentation(state, prepared, page_id, full_content);
    prepared
}

fn roots_need_presentation(roots: &[jian_ops_schema::node::PenNode]) -> bool {
    crate::initial_text_snapshot::has_content_binding(roots)
        || crate::full_content_snapshot::needs_snapshot(roots)
}

fn document_needs_presentation(doc: &jian_ops_schema::PenDocument) -> bool {
    roots_need_presentation(&doc.children)
        || doc.pages.as_ref().is_some_and(|pages| {
            pages
                .iter()
                .any(|page| roots_need_presentation(&page.children))
        })
}

fn roots_with_presentation<'a>(
    state: &op_editor_core::EditorState,
    roots: std::borrow::Cow<'a, [jian_ops_schema::node::PenNode]>,
    page_id: Option<&str>,
    full_content: bool,
) -> std::borrow::Cow<'a, [jian_ops_schema::node::PenNode]> {
    if !full_content
        || state.editor_ui.preserve_authored_geometry
        || !roots_need_presentation(&roots)
    {
        return roots;
    }
    let mut nodes = roots.into_owned();
    prepare_presentation(&mut nodes, &state.doc, page_id);
    std::borrow::Cow::Owned(nodes)
}

fn prepare_presentation(
    roots: &mut Vec<jian_ops_schema::node::PenNode>,
    doc: &jian_ops_schema::PenDocument,
    page_id: Option<&str>,
) {
    crate::initial_text_snapshot::apply(roots, doc, page_id);
    if let Some(nodes) = crate::full_content_snapshot::roots(roots) {
        *roots = nodes;
    }
}

#[cfg(test)]
#[path = "presentation_borrow_tests.rs"]
mod presentation_borrow_tests;
