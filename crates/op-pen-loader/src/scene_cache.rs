//! Skip the layout-scene rebuild when its inputs are unchanged.
//!
//! [`editor_state_to_active_page_layout_scene`](crate::editor_state_to_active_page_layout_scene)
//! is a pure function of just the document, the authored-geometry latch, the
//! active page index, normal table reading posture, and the resolved variable table (which folds the active
//! theme and the transient fill / stroke ref caches). It drops every other piece
//! of editor state — selection, hover, chat, viewport, history. Yet the host
//! marks the scene dirty on nearly every interaction: hover (each mouse-move),
//! scroll / zoom, selection / marquee, caret + uncommitted-text drafts (each
//! keystroke), UI toggles, and streamed chat / codegen deltas.
//!
//! This cache holds compact identities for those inputs and skips the rebuild
//! when they still match. Document identity is the editor's
//! `(document_generation, document_revision)` pair, so a UI-only refresh is
//! `O(variables)` rather than cloning and comparing the entire node tree.

use crate::scene_inputs::SceneInputs;
use jian_scene::layout_scene::LayoutScene;

/// The inputs of the last layout-scene build, retained so an unchanged refresh
/// can skip the (taffy + reshape) rebuild.
#[derive(Default)]
pub struct SceneBuildCache {
    last: Option<SceneInputs>,
}

impl SceneBuildCache {
    pub fn new() -> Self {
        Self { last: None }
    }

    /// Build a fresh [`LayoutScene`] only if the scene-relevant inputs (doc,
    /// active theme, active page) changed since the last build.
    ///
    /// Returns `Some(scene)` when a rebuild happened (the caller installs it),
    /// or `None` when the inputs are identical (the caller keeps its current
    /// scene).
    pub fn maybe_rebuild(&mut self, state: &op_editor_core::EditorState) -> Option<LayoutScene> {
        let Some(inputs) = SceneInputs::capture(state) else {
            self.last = None;
            return Some(crate::editor_state_to_active_page_layout_scene(state));
        };
        if self.last.as_ref().is_some_and(|last| last.matches(&inputs)) {
            return None;
        }
        let scene = crate::editor_state_to_active_page_layout_scene(state);
        // A font import may race construction without changing the document.
        // Never let that result satisfy a later request under the old key.
        self.last = inputs.fonts_stable().then_some(inputs);
        Some(scene)
    }

    /// Forget the cached inputs so the next [`maybe_rebuild`](Self::maybe_rebuild)
    /// always rebuilds. Call when the host installs a scene through a path that
    /// bypasses this cache. Whole-state replacement paths must also call this:
    /// a fresh state may restart at the same generation/revision pair as the
    /// document it replaced even though its contents differ.
    pub fn invalidate(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use op_editor_core::EditorState;

    fn state_with_rect(x: f64) -> EditorState {
        let doc = jian_ops_schema::load_str(&format!(
            r#"{{"version":"1.0.0","children":[
              {{"type":"rectangle","id":"r","name":"r","x":{x},"y":0,"width":40,"height":20}}
            ]}}"#
        ))
        .expect("fixture parses")
        .value;
        EditorState::from_document(doc)
    }

    #[test]
    fn first_build_returns_a_scene_then_unchanged_refreshes_are_skipped() {
        let mut cache = SceneBuildCache::new();
        let state = state_with_rect(0.0);

        assert!(
            cache.maybe_rebuild(&state).is_some(),
            "first build must produce a scene"
        );
        assert!(
            cache.maybe_rebuild(&state).is_none(),
            "an identical refresh must skip the rebuild"
        );
        assert!(
            cache.maybe_rebuild(&state).is_none(),
            "still skipped on repeat"
        );
    }

    #[test]
    fn a_document_change_forces_a_rebuild() {
        let mut cache = SceneBuildCache::new();
        let a = state_with_rect(0.0);
        let mut b = state_with_rect(100.0); // node moved → different doc
        b.mark_document_changed();

        assert!(cache.maybe_rebuild(&a).is_some());
        assert!(cache.maybe_rebuild(&a).is_none());
        assert!(
            cache.maybe_rebuild(&b).is_some(),
            "a moved node must rebuild the scene"
        );
        assert!(cache.maybe_rebuild(&b).is_none());
    }

    #[test]
    fn active_page_and_theme_changes_force_a_rebuild() {
        let mut cache = SceneBuildCache::new();
        let mut state = state_with_rect(0.0);
        assert!(cache.maybe_rebuild(&state).is_some());
        assert!(cache.maybe_rebuild(&state).is_none());

        state.ui.active_page_index = state.ui.active_page_index.wrapping_add(1);
        assert!(
            cache.maybe_rebuild(&state).is_some(),
            "an active-page switch must rebuild"
        );

        state.ui.active_page_index = state.ui.active_page_index.wrapping_sub(1);
        let _ = cache.maybe_rebuild(&state);
        state
            .ui
            .variables
            .active_theme
            .insert("mode".to_string(), "dark".to_string());
        assert!(
            cache.maybe_rebuild(&state).is_some(),
            "an active-theme change must rebuild (it re-resolves token fills)"
        );
    }

    #[test]
    fn toggling_preserve_authored_geometry_forces_a_rebuild() {
        // Preview mode flips `preserve_authored_geometry`, which switches the
        // layout-resolution path — so it must rebuild even though the document,
        // theme, and page are unchanged.
        let mut cache = SceneBuildCache::new();
        let mut state = state_with_rect(0.0);
        assert!(cache.maybe_rebuild(&state).is_some());
        assert!(cache.maybe_rebuild(&state).is_none());

        state.editor_ui.preserve_authored_geometry = !state.editor_ui.preserve_authored_geometry;
        assert!(
            cache.maybe_rebuild(&state).is_some(),
            "toggling preserve_authored_geometry must rebuild the scene"
        );
    }

    #[test]
    fn invalidate_forces_the_next_build() {
        let mut cache = SceneBuildCache::new();
        let state = state_with_rect(0.0);
        assert!(cache.maybe_rebuild(&state).is_some());
        assert!(cache.maybe_rebuild(&state).is_none());
        cache.invalidate();
        assert!(
            cache.maybe_rebuild(&state).is_some(),
            "invalidate must force the next rebuild"
        );
    }
}
