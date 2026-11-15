//! Text conversion, separated from the converter dispatch spine.

use crate::common::{common_props, resolve_height, resolve_width, ConversionContext};
use crate::mappers::{map_figma_effects, map_figma_fills};
use crate::node_build::text_node;
use crate::text_mapper::map_figma_text_props;
use crate::tree::TreeNode;
use jian_ops_schema::node::PenNode;
use jian_ops_schema::sizing::SizingBehavior;

mod lists;

fn is_string_sizing(s: &SizingBehavior) -> bool {
    matches!(
        s,
        SizingBehavior::Keyword(_) | SizingBehavior::Expression(_)
    )
}

pub(super) fn convert_text(
    tree: &TreeNode,
    parent_stack_mode: Option<&str>,
    ctx: &mut ConversionContext<'_>,
) -> PenNode {
    let id = ctx.generate_id();
    let figma = &tree.figma;
    let mut props = map_figma_text_props(figma);
    let width = resolve_width(figma, parent_stack_mode, ctx);
    let height = resolve_height(figma, parent_stack_mode, ctx);

    if let Some(list) = lists::convert_preserved_list(figma, &id, &width, &height, ctx) {
        return list;
    }

    // Reconcile textGrowth with the resolved width.
    use jian_ops_schema::node::text::TextGrowth;
    if props.text_growth.is_none() {
        if is_string_sizing(&width) || figma.get_str("textAutoResize").is_none() {
            props.text_growth = Some(TextGrowth::FixedWidth);
        }
    } else if props.text_growth == Some(TextGrowth::Auto) && is_string_sizing(&width) {
        props.text_growth = Some(TextGrowth::FixedWidth);
    }

    text_node(
        common_props(figma, id),
        width,
        height,
        props,
        map_figma_fills(figma.get_array("fillPaints")),
        map_figma_effects(figma.get_array("effects")),
    )
}
