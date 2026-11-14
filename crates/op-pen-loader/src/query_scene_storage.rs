//! Conservative owned-capacity charges, not allocator RSS or transient build peaks.
use crate::scene_inputs::SceneInputs;
use jian_scene::layout_scene::*;
use op_editor_core::scene_vars::{VariableScalar, VariableValue};
use std::{
    collections::{BTreeMap, HashSet},
    mem::size_of,
    sync::Arc,
};

fn map_bytes<V>(map: &BTreeMap<String, V>, value: impl Fn(&V) -> usize) -> usize {
    if map.is_empty() {
        return 0;
    }
    // Sparse root and per-entry slack cover BTree node spare slots/pointers.
    1024usize
        .saturating_add(map.len().saturating_mul(size_of::<(String, V)>() + 128))
        .saturating_add(
            map.iter()
                .map(|(key, v)| key.capacity().saturating_add(value(v)))
                .sum::<usize>(),
        )
}
fn scalar(value: &VariableScalar) -> usize {
    match value {
        VariableScalar::Str(s) => s.capacity(),
        VariableScalar::Bool(_) | VariableScalar::Num(_) => 0,
    }
}
pub(super) fn input_bytes(input: &SceneInputs) -> usize {
    let v = &input.variables;
    size_of::<SceneInputs>()
        + 32
        + v.variables.capacity() * size_of::<op_editor_core::scene_vars::Variable>()
        + v.variables
            .iter()
            .map(|variable| {
                variable.name.capacity()
                    + match &variable.value {
                        VariableValue::Scalar(v) => scalar(v),
                        VariableValue::Themed(values) => {
                            values.capacity() * size_of::<op_editor_core::scene_vars::ThemedValue>()
                                + values
                                    .iter()
                                    .map(|v| {
                                        scalar(&v.value)
                                            + v.theme
                                                .as_ref()
                                                .map_or(0, |m| map_bytes(m, String::capacity))
                                    })
                                    .sum::<usize>()
                        }
                    }
            })
            .sum::<usize>()
        + v.themes.capacity() * size_of::<op_editor_core::scene_vars::ThemeAxis>()
        + v.themes
            .iter()
            .map(|theme| {
                theme.name.capacity()
                    + theme.values.capacity() * size_of::<String>()
                    + theme.values.iter().map(String::capacity).sum::<usize>()
            })
            .sum::<usize>()
        + map_bytes(&v.active_theme, String::capacity)
        + [&v.fill_refs, &v.stroke_refs]
            .into_iter()
            .map(|refs| {
                refs.capacity() * size_of::<(String, String)>()
                    + refs
                        .iter()
                        .map(|(id, v)| id.capacity() + v.capacity())
                        .sum::<usize>()
            })
            .sum::<usize>()
        + input.components.capacity() * size_of::<(String, usize)>()
        + input
            .components
            .iter()
            .map(|(id, _)| id.capacity())
            .sum::<usize>()
}

struct Charge {
    arcs: HashSet<usize>,
}
impl Charge {
    fn arc(&mut self, value: &Arc<str>) -> usize {
        let id = Arc::as_ptr(value) as *const () as usize;
        // Bound measurement scratch. Beyond this unique-asset count, charge
        // every reference conservatively rather than grow a retained index.
        if self.arcs.len() < 65536 && !self.arcs.insert(id) {
            return 0;
        }
        value.len().saturating_add(32)
    }
    fn optional_arc(&mut self, value: Option<&Arc<str>>) -> usize {
        value.map_or(0, |v| self.arc(v))
    }
    fn gradient(&self, gradient: &SceneGradient) -> usize {
        match gradient {
            SceneGradient::Linear { stops, .. } | SceneGradient::Radial { stops, .. } => {
                stops.capacity() * size_of::<SceneGradientStop>()
            }
            SceneGradient::Mesh { colors, .. } => {
                colors.capacity() * size_of::<op_editor_core::render_backend::Color>()
            }
        }
    }
    fn shader(&self, shader: &SceneShader) -> usize {
        let gradient_bytes = shader.gradient.as_ref().map_or(0, |paint| {
            std::mem::size_of::<jian_ops_schema::style::GradientPaint>()
                + 2 * std::mem::size_of::<usize>()
                + paint.stops.capacity()
                    * std::mem::size_of::<jian_ops_schema::style::GradientStop>()
                + paint
                    .stops
                    .iter()
                    .map(|stop| stop.color.capacity())
                    .sum::<usize>()
        });
        gradient_bytes
            + shader.sksl.capacity()
            + shader.uniforms.capacity() * size_of::<SceneShaderUniform>()
            + shader
                .uniforms
                .iter()
                .map(|u| u.name.capacity() + u.values.capacity() * size_of::<f32>())
                .sum::<usize>()
    }
    fn widget(&self, widget: &SceneWidget) -> usize {
        widget.kind.capacity()
            + [
                &widget.value_str,
                &widget.placeholder,
                &widget.leading_icon,
                &widget.trailing_icon,
                &widget.label,
            ]
            .into_iter()
            .map(|s| s.as_ref().map_or(0, String::capacity))
            .sum::<usize>()
            + widget.options.capacity() * size_of::<SceneWidgetOption>()
            + widget
                .options
                .iter()
                .map(|o| o.value.capacity() + o.label.capacity())
                .sum::<usize>()
    }
    fn nodes(&mut self, nodes: &Vec<SceneNode>) -> usize {
        nodes.capacity() * size_of::<SceneNode>()
            + nodes.iter().map(|node| self.node(node)).sum::<usize>()
    }
    fn node(&mut self, node: &SceneNode) -> usize {
        scene_shape_guard(node);
        node.id.capacity()
            + match &node.kind {
                NodeKind::Other(s) => s.capacity(),
                _ => 0,
            }
            + node.font_family.capacity()
            + node.text.as_ref().map_or(0, String::capacity)
            + node.svg_path.as_ref().map_or(0, String::capacity)
            + node.text_runs.capacity() * size_of::<SceneTextRun>()
            + node.points.capacity() * size_of::<op_editor_core::render_backend::Point2D>()
            + node.path_anchors.capacity() * size_of::<SceneAnchor>()
            + node.effects.capacity() * size_of::<Effect>()
            + map_bytes(&node.font_features, |_| 0)
            + node.gradient.as_ref().map_or(0, |v| self.gradient(v))
            + node.shader.as_ref().map_or(0, |v| self.shader(v))
            + node.widget.as_ref().map_or(0, |v| self.widget(v))
            + self.optional_arc(node.image_src.as_ref())
            + self.optional_arc(node.image_thumbnail_src.as_ref())
            + node.video.as_ref().map_or(0, |v| self.arc(&v.src))
            + node.fill_layers.capacity() * size_of::<SceneFillLayer>()
            + node
                .fill_layers
                .iter()
                .map(|layer| match layer {
                    SceneFillLayer::Solid { .. } => 0,
                    SceneFillLayer::Gradient { gradient, .. } => self.gradient(gradient),
                    SceneFillLayer::Shader { shader, .. } => self.shader(shader),
                    SceneFillLayer::Image {
                        src, thumbnail_src, ..
                    } => self.arc(src) + self.optional_arc(thumbnail_src.as_ref()),
                })
                .sum::<usize>()
            + self.nodes(&node.children)
    }
}
pub(super) fn scene_bytes(scene: &LayoutScene) -> usize {
    let mut charge = Charge {
        arcs: HashSet::new(),
    };
    size_of::<LayoutScene>()
        + 32
        + scene.pages.capacity() * size_of::<ScenePage>()
        + scene
            .pages
            .iter()
            .map(|page| page.id.capacity() + page.name.capacity() + charge.nodes(&page.children))
            .sum::<usize>()
}

// Adding a scene field requires revisiting its dynamic storage charge.
fn scene_shape_guard(node: &SceneNode) {
    let SceneNode {
        id: _,
        kind: _,
        bounds: _,
        aggregate_bounds_cache: _,
        opacity: _,
        composite_opacity: _,
        blend_mode: _,
        rotation: _,
        flip_x: _,
        flip_y: _,
        corner_radius: _,
        corner_radii: _,
        clip_content: _,
        fill: _,
        fill_layers: _,
        fill_type: _,
        gradient: _,
        shader: _,
        stroke: _,
        text: _,
        text_runs: _,
        font_family: _,
        font_size: _,
        font_weight: _,
        font_kerning: _,
        font_features: _,
        italic: _,
        underline: _,
        strikethrough: _,
        text_decoration_skip_ink: _,
        line_height: _,
        letter_spacing: _,
        text_align: _,
        text_vertical_align: _,
        text_wrap: _,
        points: _,
        path_anchors: _,
        path_closed: _,
        is_mask: _,
        mask_type: _,
        even_odd_fill: _,
        svg_path: _,
        arc_start_angle: _,
        arc_sweep_angle: _,
        arc_inner_radius: _,
        polygon_sides: _,
        image_src: _,
        video: _,
        image_src_id: _,
        image_thumbnail_src: _,
        image_thumbnail_src_id: _,
        image_nearest_sampling: _,
        image_fit: _,
        image_blend_mode: _,
        image_transform: _,
        image_rotation_degrees: _,
        image_original_size: _,
        image_tile_scale: _,
        image_adjustments: _,
        effects: _,
        hidden: _,
        locked: _,
        widget: _,
        children: _,
    } = node;
}
