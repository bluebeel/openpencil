//! Clone one canonical node's metadata without materializing its descendant graph.
use jian_ops_schema::node::{FrameNode, GroupNode, PenNode, RectangleNode, RefNode, TabsNode};

/// Preserve every non-child field and None versus Some(empty) child presence.
/// Explicit struct initializers force a compile error if container fields grow.
#[allow(clippy::clone_on_copy)]
pub fn clone_without_children(node: &PenNode) -> PenNode {
    macro_rules! copy {
        ($value:ident, $ty:ident, $($field:ident),+ $(,)?) => {
            $ty { children: $value.children.as_ref().map(|_| Vec::new()),
                $($field: $value.$field.clone()),+ }
        };
    }
    match node {
        PenNode::Frame(n) => PenNode::Frame(copy!(
            n,
            FrameNode,
            base,
            container,
            image_search_query,
            reusable,
            slot,
            state,
            bindings,
            events,
            lifecycle,
            semantics,
            gestures,
            route,
            screen,
            breakpoint
        )),
        PenNode::Group(n) => PenNode::Group(copy!(
            n, GroupNode, base, container, state, bindings, events, lifecycle, semantics, gestures,
            route
        )),
        PenNode::Rectangle(n) => PenNode::Rectangle(copy!(
            n,
            RectangleNode,
            base,
            container,
            state,
            bindings,
            events,
            lifecycle,
            semantics,
            gestures,
            route
        )),
        PenNode::Tabs(n) => PenNode::Tabs(copy!(
            n,
            TabsNode,
            base,
            width,
            height,
            limits,
            tabs,
            value,
            fill,
            stroke,
            effects,
            corner_radius,
            states,
            state,
            bindings,
            events,
            lifecycle,
            semantics,
            gestures,
            route
        )),
        PenNode::Ref(n) => PenNode::Ref(copy!(
            n,
            RefNode,
            base,
            target,
            descendants,
            state,
            bindings,
            events,
            lifecycle,
            semantics,
            gestures,
            route
        )),
        other => other.clone(),
    }
}

#[cfg(test)]
#[path = "node_shallow_tests.rs"]
mod tests;
