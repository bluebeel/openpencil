use super::*;

pub fn collect_batch_design_diagnostics(state: &EditorState) -> BatchDesignDiagnostics {
    let scene = op_pen_loader::editor_state_to_active_page_layout_scene(state);
    collect_batch_design_diagnostics_with_scene(state, &scene)
}

/// Reuse one matching immutable layout without rebuilding its full paint tree.
pub fn collect_batch_design_diagnostics_with_scene(
    state: &EditorState,
    scene: &op_editor_ui::layout_scene::LayoutScene,
) -> BatchDesignDiagnostics {
    let effective_theme = op_editor_core::variables_resolve::effective_theme(
        &state.doc,
        &state.ui.variables.active_theme,
    );
    collect_batch_design_diagnostics_with_scene_and_theme(state, scene, &effective_theme)
}
/// Same report, borrowing the already derived effective theme of the enclosing
/// quality scan. Public fresh entrypoints still compute their current table.
pub(crate) fn collect_batch_design_diagnostics_with_scene_and_theme(
    state: &EditorState,
    scene: &op_editor_ui::layout_scene::LayoutScene,
    effective_theme: &BTreeMap<String, String>,
) -> BatchDesignDiagnostics {
    let rects = resolved_sizes(scene);
    let mut out = BatchDesignDiagnostics::default();
    for root in state.active_children() {
        walk(
            root,
            &rects,
            state.doc.variables.as_ref(),
            effective_theme,
            &mut out,
        );
    }
    out.layout_issues.truncate(LIMIT);
    out.intent_questions.truncate(LIMIT);
    out.variable_issues.truncate(LIMIT);
    out.image_slot_candidates.truncate(LIMIT);
    out
}

fn resolved_sizes(scene: &op_editor_ui::layout_scene::LayoutScene) -> HashMap<String, (f64, f64)> {
    let mut out = HashMap::new();
    fn walk(
        nodes: &[op_editor_ui::layout_scene::SceneNode],
        out: &mut HashMap<String, (f64, f64)>,
    ) {
        for node in nodes {
            let bounds = node.aggregate_bounds();
            out.insert(
                node.id.clone(),
                (bounds.size.x as f64, bounds.size.y as f64),
            );
            walk(&node.children, out);
        }
    }
    if let Some(page) = scene.active_page() {
        walk(&page.children, &mut out);
    }
    out
}
