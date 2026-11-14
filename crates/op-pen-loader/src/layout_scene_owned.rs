//! Consuming payload-to-scene conversion. Canonical source stays borrowed;
//! the already-owned intermediate payload yields its text/path/font buffers
//! directly, and converted child/page payloads leave as the traversal advances.
use super::*;

pub(crate) fn payload_into_scene(
    payload: DocPayload,
    var_table: &VariableTable,
    active_page_index: usize,
) -> LayoutScene {
    let active_page_index = active_page_index.min(payload.pages.len().saturating_sub(1));
    LayoutScene {
        pages: payload
            .pages
            .into_iter()
            .map(|page| ScenePage {
                id: page.id,
                name: page.name,
                children: page
                    .children
                    .into_iter()
                    .map(|node| node_payload_to_scene(node, var_table, 1.0))
                    .collect(),
            })
            .collect(),
        active_page_index,
    }
}

pub(crate) fn node_payload_to_scene(
    node: NodePayload,
    var_table: &VariableTable,
    inherited_paint_opacity: f32,
) -> SceneNode {
    use op_editor_core::render_backend::{Point2D, Rect};
    let node_id = op_editor_core::NodeId::new(node.id.clone());
    let mask_type = node.mask_type.or_else(|| {
        node.is_mask
            .then_some(jian_ops_schema::node::MaskType::Alpha)
    });
    // A mask source is composited into siblings that already carry the common
    // ancestor opacity. Baking that ancestor into the mask as well would
    // multiply it a second time through DstIn. Reset only at the mask root;
    // Its own local opacity remains represented on either the direct paint or
    // the source's isolation layer.
    let inherited_paint_opacity = if mask_type.is_some() {
        1.0
    } else {
        inherited_paint_opacity
    };
    let local_opacity = node.opacity.clamp(0.0, 1.0);
    let blend_mode = blend_mode_to_scene(node.blend_mode.as_ref());
    // A translucent subtree must apply its local alpha after its own paint and
    // children have assembled; otherwise overlapping children accumulate alpha
    // independently. Non-Normal node blending already requires the same
    // isolation layer, so it carries local opacity there even for leaves. Keep
    // ordinary leaves on the direct-paint path to avoid allocating a layer.
    let isolates_output =
        blend_mode != ImageBlendMode::Normal || (!node.children.is_empty() && local_opacity < 1.0);
    let paint_opacity = if isolates_output {
        inherited_paint_opacity
    } else {
        (inherited_paint_opacity * local_opacity).clamp(0.0, 1.0)
    };
    let composite_opacity = if isolates_output { local_opacity } else { 1.0 };
    let bounds = Rect {
        origin: Point2D::new(node.x, node.y),
        size: Point2D::new(node.w, node.h),
    };
    let variable_fill = var_table.fill_for(&node_id);
    let stroke = if is_unpainted_widget_stroke(&node, &node_id, var_table) {
        // A first-class widget's stroke is its *inactive track / border*
        // role paint, not a literal outline. When the author declared a
        // stroke with no resolvable colour, handing the painter a
        // fabricated opaque black turned every such control into a black
        // track / border; dropping it lets
        // `resolve_authored_widget_visual` fall back to its role defaults
        // (derived-from-fill or the legacy #D1D5DB track, and a
        // borderless select) exactly as an unstroked control does.
        None
    } else if is_status_bar_shell_stroke(&node) {
        // The scene path (editor canvas + render-shots) bypasses the
        // adapter's `legacy_payload_repair`, so an iPhone status-bar
        // shell ("Time"/"Levels") authored with a no-fill stroke would
        // paint a phantom black box around the clock / signal cluster —
        // invisible in Pencil. Drop the stroke the same way here.
        None
    } else {
        node.stroke.as_ref().map(|s| {
            let mut st = scene_stroke(s, &node_id, var_table);
            st.color = mul_alpha(st.color, paint_opacity);
            st
        })
    };
    let fill_layers = fill_layers_to_scene(&node.fill_layers, variable_fill, [node.w, node.h]);
    drop(node.fill_layers);
    let text_runs = text_runs_to_scene(&node.text_runs, paint_opacity);
    drop(node.text_runs);
    let children: Vec<SceneNode> = node
        .children
        .into_iter()
        .map(|c| node_payload_to_scene(c, var_table, paint_opacity))
        .collect();
    let aggregate_bounds_cache = SceneNode::compute_aggregate_bounds(bounds, &children);
    SceneNode {
        id: node.id,
        kind: str_to_kind(&node.kind),
        bounds,
        aggregate_bounds_cache,
        // Carried so the image and canonical fill-stack painters can apply
        // direct paint opacity; legacy fill/stroke/gradient/shadow fields
        // already have `paint_opacity` folded into their alpha below.
        opacity: paint_opacity,
        composite_opacity,
        blend_mode,
        rotation: node.rotation,
        flip_x: node.flip_x,
        flip_y: node.flip_y,
        corner_radius: node.corner_radius,
        corner_radii: node.corner_radii,
        clip_content: node.clip_content,
        // Paint-time `$ref` resolution: a registered fill ref wins,
        // else the node's own fill. Same precedence as the canvas
        // painter's `node_fill` helper.
        fill: variable_fill
            .or_else(|| node.fill.map(array_to_color))
            .map(|c| mul_alpha(c, paint_opacity)),
        fill_layers,
        fill_type: str_to_scene_fill_type(&node.fill_type),
        gradient: node
            .gradient
            .map(|g| scale_gradient_opacity(payload_gradient_into_scene(g), paint_opacity)),
        shader: node
            .shader
            .map(|s| payload_shader_into_scene(s, paint_opacity, [node.w, node.h])),
        stroke,
        text: node.text,
        text_runs,
        font_family: node.font_family,
        font_size: node.font_size,
        font_weight: node.font_weight,
        font_kerning: node.font_kerning,
        font_features: node.font_features,
        italic: node.italic,
        underline: node.underline,
        strikethrough: node.strikethrough,
        text_decoration_skip_ink: node.text_decoration_skip_ink,
        line_height: node.line_height,
        letter_spacing: node.letter_spacing,
        text_align: text_align_to_scene(&node.text_align),
        text_vertical_align: text_vertical_align_to_scene(&node.text_vertical_align),
        text_wrap: node.text_wrap,
        points: node
            .points
            .into_iter()
            .map(|p| Point2D::new(p[0], p[1]))
            .collect(),
        path_anchors: node
            .path_anchors
            .into_iter()
            .map(|anchor| anchor_to_scene(&anchor))
            .collect(),
        path_closed: node.path_closed,
        is_mask: node.is_mask || node.mask_type.is_some(),
        mask_type,
        even_odd_fill: node.even_odd_fill,
        svg_path: node.svg_path,
        arc_start_angle: node.arc_start_angle,
        arc_sweep_angle: node.arc_sweep_angle,
        arc_inner_radius: node.arc_inner_radius,
        polygon_sides: node.polygon_sides.clamp(3, 100),
        image_src: node.image_src.as_ref().map(|s| s.as_arc()),
        image_thumbnail_src: node.image_thumbnail_src.as_ref().map(|src| src.as_arc()),
        image_thumbnail_src_id: node
            .image_thumbnail_src
            .as_deref()
            .map(stable_image_source_id)
            .unwrap_or(0),
        image_nearest_sampling: node.image_nearest_sampling.unwrap_or(false),
        image_rotation_degrees: node.image_rotation_degrees,
        video: node.video.map(|video| SceneVideo {
            src: Arc::from(video.src),
            autoplay: video.autoplay,
            r#loop: video.r#loop,
            muted: video.muted,
            hold_last_frame: video.hold_last_frame,
            click_to_replay: video.click_to_replay,
        }),
        image_src_id: node
            .image_src
            .as_deref()
            .map(stable_image_source_id)
            .unwrap_or(0),
        image_fit: image_fit_to_scene(node.image_fit.as_deref()),
        image_blend_mode: blend_mode_to_scene(node.image_blend_mode.as_ref()),
        image_transform: node.image_transform,
        image_original_size: image_original_size_to_scene(node.image_original_size),
        image_tile_scale: image_tile_scale_to_scene(node.image_tile_scale),
        image_adjustments: image_adjustments_to_scene(node.image_adjustments),
        effects: crate::effects::effects_from_payload(node.effects)
            .into_iter()
            .chain(crate::effects::blur_effect_from_payload(node.layer_blur))
            .chain(crate::effects::background_blur_effect_from_payload(
                node.background_blur,
            ))
            .map(|e| scale_effect_opacity(e, paint_opacity))
            .collect(),
        hidden: node.hidden,
        locked: node.locked,
        // Widget props are already concrete after the adapter harvested them;
        // only the adjacent-label page colour comes from the variable table.
        widget: node
            .widget
            .as_ref()
            .map(|w| widget_payload_to_scene(w, var_table)),
        children,
    }
}

pub(super) fn payload_gradient_into_scene(g: GradientPayload) -> SceneGradient {
    match g {
        GradientPayload::Linear {
            angle_deg,
            opacity,
            stops,
        } => SceneGradient::Linear {
            angle_deg,
            opacity,
            stops: stops.into_iter().map(|stop| stop_to_scene(&stop)).collect(),
        },
        GradientPayload::Radial {
            cx,
            cy,
            radius,
            opacity,
            stops,
        } => SceneGradient::Radial {
            cx,
            cy,
            radius,
            opacity,
            stops: stops.into_iter().map(|stop| stop_to_scene(&stop)).collect(),
        },
        GradientPayload::Mesh {
            rows,
            cols,
            colors,
            opacity,
        } => SceneGradient::Mesh {
            rows,
            cols,
            colors: colors.into_iter().map(array_to_color).collect(),
            opacity,
        },
    }
}

pub(super) fn payload_shader_into_scene(s: ShaderPayload, k: f32, size: [f32; 2]) -> SceneShader {
    let mut uniforms: Vec<SceneShaderUniform> = s
        .uniforms
        .into_iter()
        .map(|uniform| SceneShaderUniform {
            name: uniform.name,
            values: uniform.values,
        })
        .collect();
    if let Some(uniform) = uniforms.iter_mut().find(|uniform| uniform.name == "size") {
        uniform.values = size.to_vec();
    }
    SceneShader {
        gradient: s.gradient.map(std::sync::Arc::from),
        sksl: s.sksl,
        uniforms,
        opacity: (s.opacity * k).clamp(0.0, 1.0),
        fallback: array_to_color(s.fallback),
    }
}
