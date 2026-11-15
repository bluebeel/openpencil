use super::*;

/// Build a `Path` PenNode for a host-resolved icon — ports
/// `path-converter.ts` lines 25-65. Stroke gets a sensible default
/// when the node has no stroke paint, and the stroke thickness scales
/// down proportionally for icons smaller than the lucide 24×24
/// reference box.
pub(super) fn build_icon_path_node(
    figma: &FigValue,
    id: String,
    parent_stack_mode: Option<&str>,
    ctx: &mut ConversionContext<'_>,
    icon: crate::common::IconLookupResult,
) -> PenNode {
    use jian_ops_schema::style::{
        PenFill, PenStroke, SolidFillBody, StrokeCap, StrokeJoin, StrokeThickness,
    };

    let icon_w = resolve_width(figma, parent_stack_mode, ctx);
    let icon_h = resolve_height(figma, parent_stack_mode, ctx);

    // iconSize = min(width-if-numeric, height-if-numeric), defaulting to
    // 24 (the lucide reference box) when either axis is non-numeric.
    let w_num: f64 = match icon_w {
        SizingBehavior::Number(n) => n,
        _ => 24.0,
    };
    let h_num: f64 = match icon_h {
        SizingBehavior::Number(n) => n,
        _ => 24.0,
    };
    let icon_size = w_num.min(h_num);
    let icon_scale: f64 = icon_size / 24.0;

    let style = icon.style.unwrap_or(IconStyle::Stroke);
    let mapped_stroke = map_figma_stroke(figma);
    let mut stroke = match style {
        IconStyle::Stroke => Some(mapped_stroke.unwrap_or_else(|| {
            PenStroke::from(jian_ops_schema::style::PenStrokeBody {
                thickness: StrokeThickness::Uniform(1.5),
                align: None,
                join: Some(StrokeJoin::Round),
                cap: Some(StrokeCap::Round),
                dash_pattern: None,
                dash_offset: None,
                fill: if figma.get_array("fillPaints").is_some_and(|paints| {
                    !paints.is_empty()
                        && paints
                            .iter()
                            .all(|paint| paint.get_bool("visible") == Some(false))
                }) {
                    map_figma_fills(figma.get_array("fillPaints"))
                } else {
                    Some(vec![PenFill::Solid(SolidFillBody {
                        enabled: None,
                        color: fig_fill_color(figma).unwrap_or_else(|| "#000000".to_string()),
                        explain: None,
                        opacity: None,
                        blend_mode: None,
                    })])
                },
            })
        })),
        IconStyle::Fill => mapped_stroke,
    };

    if let Some(s) = stroke.as_mut() {
        if icon_scale < 0.99 {
            if let StrokeThickness::Uniform(t) = s.thickness {
                let scaled = round2(t * icon_scale);
                s.thickness = StrokeThickness::Uniform(scaled);
            }
        }
    }

    let fill = match style {
        IconStyle::Fill => map_figma_fills(figma.get_array("fillPaints")),
        IconStyle::Stroke => None,
    };

    path_node(
        common_props(figma, id),
        Some(icon.d),
        icon.icon_id,
        None,
        icon_w,
        icon_h,
        fill,
        stroke,
        map_figma_node_effects(figma),
        figma_path_mask(figma),
    )
}
