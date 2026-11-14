//! The exact interactive scene inputs, shared by local and trusted-host caches.
use op_editor_core::{
    scene_vars::{ThemeAxis, Variable, VariableTable},
    EditorState,
};
use std::{collections::BTreeMap, sync::Weak};

#[derive(PartialEq)]
pub(crate) struct Variables {
    pub(super) variables: Vec<Variable>,
    pub(super) themes: Vec<ThemeAxis>,
    pub(super) active_theme: BTreeMap<String, String>,
    pub(super) fill_refs: Vec<(String, String)>,
    pub(super) stroke_refs: Vec<(String, String)>,
}
impl From<VariableTable> for Variables {
    fn from(table: VariableTable) -> Self {
        // Move NodeId's actual String allocation rather than estimate hidden
        // capacities. Sorted maps preserve VariableTable's order-independent Eq.
        let refs = |map: std::collections::HashMap<op_editor_core::NodeId, String>| {
            let mut refs = map
                .into_iter()
                .map(|(id, value)| (String::from(id), value))
                .collect::<Vec<_>>();
            refs.sort_unstable();
            refs
        };
        Self {
            variables: table.variables,
            themes: table.themes,
            active_theme: table.active_theme,
            fill_refs: refs(table.fill_refs),
            stroke_refs: refs(table.stroke_refs),
        }
    }
}

pub(crate) struct SceneInputs {
    pub(super) owner: Weak<()>,
    generation: u64,
    revision: u64,
    active_page: usize,
    preserve: bool,
    normal_table: bool,
    normal_presentation: bool,
    preview: bool,
    pub(super) variables: Variables,
    pub(super) font_generation: u64,
    pub(super) components: Vec<(String, usize)>,
}
impl SceneInputs {
    pub(crate) fn capture(state: &EditorState) -> Option<Self> {
        let mut components = Vec::new();
        if state.document_revision() == 0 {
            // The fresh scene builder reads this registry only before the
            // first edit. Mutable external prototypes have no revision proof.
            for component in &state.components.components {
                if !state.components.is_document_backed(&component.id) {
                    return None;
                }
                let root = state.components.resolved_root(&state.doc, &component.id)?;
                components.push((component.id.as_str().to_owned(), root as *const _ as usize));
            }
        }
        Some(Self {
            owner: state.derived_cache_owner(),
            generation: state.document_generation(),
            revision: state.document_revision(),
            active_page: state.ui.active_page_index,
            preserve: state.editor_ui.preserve_authored_geometry,
            normal_table: state.editor_ui.workspace.visible,
            normal_presentation: crate::editor_scene::normal_presentation(state),
            preview: state.editor_ui.preview.mode,
            variables: crate::editor_state_var_table(state).into(),
            font_generation: crate::current_font_generation(),
            components,
        })
    }
    pub(crate) fn matches(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.owner, &other.owner)
            && self.generation == other.generation
            && self.revision == other.revision
            && self.active_page == other.active_page
            && self.preserve == other.preserve
            && self.normal_table == other.normal_table
            && self.normal_presentation == other.normal_presentation
            && self.preview == other.preview
            && self.font_generation == other.font_generation
            && self.variables == other.variables
            && self.components == other.components
    }
    pub(crate) fn owner_live(&self) -> bool {
        self.owner.strong_count() != 0
    }
    pub(crate) fn fonts_stable(&self) -> bool {
        self.font_generation == crate::current_font_generation()
    }
}
