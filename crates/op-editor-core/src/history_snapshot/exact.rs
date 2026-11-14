//! Exact common-leaf comparison without a redundant serializer pass.
//!
//! Exhaustive struct patterns and enum matches make classified schema changes
//! compile errors. Rich/unsupported metadata returns None, preserving the
//! original PartialEq plus sensitivity path and its map-order distinctions.
//! Floating values require ordinary equality AND identical bits (including
//! signed zero); NaNs keep the reference's non-reflexive behavior.
use jian_ops_schema::{
    node::{
        base::{NumberOrExpression, PenNodeBase},
        container::ContainerProps,
        PenNode,
    },
    sizing::{SizeLimits, SizingBehavior, SizingKeyword},
    style::{PenFill, SolidFillBody},
};
use serde::Serialize;
#[inline(always)]
fn slow<T: PartialEq + Serialize + ?Sized>(a: &T, b: &T) -> bool {
    a == b && super::sensitivity::bytes(a) == super::sensitivity::bytes(b)
}
#[inline(always)]
fn option<T>(a: &Option<T>, b: &Option<T>, same: impl FnOnce(&T, &T) -> bool) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => same(a, b),
        _ => false,
    }
}
#[inline(always)]
fn optional<T: PartialEq + Serialize>(a: &Option<T>, b: &Option<T>) -> bool {
    option(a, b, slow)
}
#[inline(always)]
fn string(a: &String, b: &String) -> bool {
    a == b
}
#[inline(always)]
fn strings(a: &Option<String>, b: &Option<String>) -> bool {
    a == b
}
#[inline(always)]
fn bools(a: &Option<bool>, b: &Option<bool>) -> bool {
    a == b
}
#[inline(always)]
fn number(a: f64, b: f64) -> bool {
    a == b && a.to_bits() == b.to_bits()
}
#[inline(always)]
fn floats(a: &Option<f64>, b: &Option<f64>) -> bool {
    option(a, b, |a, b| number(*a, *b))
}
#[inline(always)]
fn floats32(a: &Option<f32>, b: &Option<f32>) -> bool {
    option(a, b, |a, b| a == b && a.to_bits() == b.to_bits())
}
macro_rules! fields {($name:ident,$ty:path,{$($field:ident:$same:path),+$(,)?})=>{#[inline(always)] fn $name(a:&$ty,b:&$ty)->bool {let $ty{$($field:_,)+}=a;true$(&&$same(&a.$field,&b.$field))+}}}
fields!(base,PenNodeBase,{id:string,name:strings,role:strings,explain:strings,x:floats,y:floats,rotation:floats,constraints:optional,opacity:opacity,enabled:optional,visible:bools,locked:bools,pin:bools,flip_x:bools,flip_y:bools,mask_type:optional,blend_mode:optional,theme:optional,transition:optional,animations:optional});
#[inline(always)]
fn opacity(a: &Option<NumberOrExpression>, b: &Option<NumberOrExpression>) -> bool {
    option(a, b, |a, b| match a {
        NumberOrExpression::Number(x) => match b {
            NumberOrExpression::Number(y) => number(*x, *y),
            _ => false,
        },
        NumberOrExpression::Expression(x) => matches!(b,NumberOrExpression::Expression(y)if x==y),
    })
}
#[inline(always)]
fn size(a: &Option<SizingBehavior>, b: &Option<SizingBehavior>) -> bool {
    option(a, b, |a, b| match a {
        SizingBehavior::Number(x) => match b {
            SizingBehavior::Number(y) => number(*x, *y),
            _ => false,
        },
        SizingBehavior::Expression(x) => matches!(b,SizingBehavior::Expression(y)if x==y),
        SizingBehavior::Keyword(x) => match x {
            SizingKeyword::FitContent => {
                matches!(b, SizingBehavior::Keyword(SizingKeyword::FitContent))
            }
            SizingKeyword::FillContainer => {
                matches!(b, SizingBehavior::Keyword(SizingKeyword::FillContainer))
            }
        },
    })
}
fields!(limits,SizeLimits,{min_width:floats,max_width:floats,min_height:floats,max_height:floats});
fields!(solid,SolidFillBody,{color:string,explain:strings,opacity:floats32,blend_mode:optional});
#[inline(always)]
fn fill(a: &PenFill, b: &PenFill) -> bool {
    match a {
        PenFill::Solid(x) => match b {
            PenFill::Solid(y) => solid(x, y),
            _ => false,
        },
        PenFill::LinearGradient(_)
        | PenFill::RadialGradient(_)
        | PenFill::MeshGradient(_)
        | PenFill::Shader(_)
        | PenFill::Image(_) => slow(a, b),
    }
}
#[inline(always)]
fn fills(a: &Option<Vec<PenFill>>, b: &Option<Vec<PenFill>>) -> bool {
    option(a, b, |a, b| {
        a.len() == b.len() && a.iter().zip(b).all(|(a, b)| fill(a, b))
    })
}
fields!(container,ContainerProps,{width:size,height:size,limits:limits,layout:optional,gap:opacity,padding:optional,justify_content:optional,align_items:optional,clip_content:bools,sticky_children:optional,corner_radius:optional,fill:fills,stroke:optional,effects:optional});
fields!(rectangle,jian_ops_schema::node::RectangleNode,{base:base,container:container,children:optional,state:optional,bindings:optional,events:optional,lifecycle:optional,semantics:optional,gestures:optional,route:optional});
#[inline(always)]
pub(super) fn geometry_matches(a: &PenNode, b: &PenNode) -> Option<bool> {
    match a {
        PenNode::Rectangle(x) => {
            if !eligible(x) {
                return None;
            }
            match b {
                PenNode::Rectangle(y) if eligible(y) => Some(rectangle(x, y)),
                PenNode::Rectangle(_) => None,
                _ => Some(false),
            }
        }
        PenNode::Frame(_)
        | PenNode::Group(_)
        | PenNode::Ellipse(_)
        | PenNode::Line(_)
        | PenNode::Polygon(_)
        | PenNode::Path(_)
        | PenNode::Text(_)
        | PenNode::TextInput(_)
        | PenNode::Image(_)
        | PenNode::IconFont(_)
        | PenNode::TextArea(_)
        | PenNode::Select(_)
        | PenNode::Switch(_)
        | PenNode::Checkbox(_)
        | PenNode::Slider(_)
        | PenNode::RadioGroup(_)
        | PenNode::NumberInput(_)
        | PenNode::Progress(_)
        | PenNode::Tabs(_)
        | PenNode::Ref(_) => None,
    }
}

#[inline(always)]
fn eligible(n: &jian_ops_schema::node::RectangleNode) -> bool {
    let b = &n.base;
    let c = &n.container;
    n.children.is_none()
        && n.state.is_none()
        && n.bindings.is_none()
        && n.events.is_none()
        && n.lifecycle.is_none()
        && n.semantics.is_none()
        && n.gestures.is_none()
        && n.route.is_none()
        && b.constraints.is_none()
        && b.enabled.is_none()
        && b.mask_type.is_none()
        && b.blend_mode.is_none()
        && b.theme.is_none()
        && b.transition.is_none()
        && b.animations.is_none()
        && c.layout.is_none()
        && c.padding.is_none()
        && c.justify_content.is_none()
        && c.align_items.is_none()
        && c.sticky_children.is_none()
        && c.corner_radius.is_none()
        && c.stroke.is_none()
        && c.effects.is_none()
        && c.fill.as_ref().is_none_or(|v| {
            v.iter()
                .all(|f| matches!(f,PenFill::Solid(s)if s.blend_mode.is_none()))
        })
}
