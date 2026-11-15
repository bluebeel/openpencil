//! Compact geometry uses the existing1GiB scene budget/coalescing machinery,
//! rather than silently missing a32MiB cache on every large-page read.
use super::*;
pub(crate) fn computed_geometry_cached(
    state: &EditorState,
    page: usize,
    item: Option<&str>,
    parent_only: bool,
    build: impl FnOnce() -> Vec<crate::ComputedGeometry>,
) -> Arc<Vec<crate::ComputedGeometry>> {
    let roots = if state.editor_ui.workspace.visible {
        crate::item_scene::selected_page_roots(state, page, None)
    } else {
        crate::item_scene::selected_page_roots_cached(state, page, item)
    };
    let scope = roots
        .as_ref()
        .map(|(_, roots)| (roots.as_ptr() as usize, roots.len()));
    let parent = if parent_only { item } else { None };
    if let Some(scope) = scope {
        if let Some(Value::Geometry(value)) = borrowed::hit(
            state,
            |key| matches!(key,Key::Geometry{page:p,scope:s,parent:x,..} if *p==page&&*s==scope&&x.as_deref()==parent),
        ) {
            return value;
        }
    }
    let key = scope.and_then(|scope| {
        if parent.is_some_and(|p| p.len() > 4096) {
            return None;
        }
        Some(Key::Geometry {
            input: SceneInputs::capture(state)?,
            page,
            scope,
            parent: parent.map(str::to_owned),
        })
    });
    let Some(key) = key else {
        return crate::geometry_cache::fresh_geometry(build);
    };
    match get_or_build(Some(key), || Value::Geometry(Arc::new(build()))) {
        Value::Geometry(value) => value,
        _ => unreachable!("compact geometry has a distinct cache key"),
    }
}
