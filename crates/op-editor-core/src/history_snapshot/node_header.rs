//! Borrow every canonical non-child field. Exhaustive struct patterns make a
//! future container field a compile error instead of silently dropping proof.
use jian_ops_schema::node::{FrameNode, GroupNode, PenNode, RectangleNode, RefNode, TabsNode};
macro_rules! same {
    ($ty:ident,$a:ident,$b:ident,$($field:ident),+$(,)?)=>{{
        let $ty {children:_, $($field:_,)+}=$a;
        $a.children.is_some()==$b.children.is_some() $(&&$a.$field==$b.$field)+
    }};
}
pub(super) fn partial_eq(a: &PenNode, b: &PenNode) -> bool {
    match (a, b) {
        (PenNode::Frame(a), PenNode::Frame(b)) => same!(
            FrameNode,
            a,
            b,
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
        ),
        (PenNode::Group(a), PenNode::Group(b)) => same!(
            GroupNode, a, b, base, container, state, bindings, events, lifecycle, semantics,
            gestures, route
        ),
        (PenNode::Rectangle(a), PenNode::Rectangle(b)) => same!(
            RectangleNode,
            a,
            b,
            base,
            container,
            state,
            bindings,
            events,
            lifecycle,
            semantics,
            gestures,
            route
        ),
        (PenNode::Tabs(a), PenNode::Tabs(b)) => same!(
            TabsNode,
            a,
            b,
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
        ),
        (PenNode::Ref(a), PenNode::Ref(b)) => same!(
            RefNode,
            a,
            b,
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
        ),
        _ => a == b,
    }
}
