use super::*;
use crate::converters::tests::{fresh_ctx, obj, solid_paint};
use crate::text_cache::layout::{SavedBaseline, SavedMarker};
use jian_ops_schema::node::base::NumberOrExpression;

fn fixture() -> FigValue {
    let characters = "Head\nA😀 bold words";
    let layout = SavedTextLayout {
        source_size: Some((160.0, 72.0)),
        baselines: [(0, 5, 0.0, 0.0), (5, 13, 24.0, 24.0), (13, 19, 24.0, 48.0)]
            .into_iter()
            .map(|(first, end, x, line_y)| SavedBaseline {
                first,
                end,
                x,
                line_y,
                y: line_y + 17.5,
                line_height: 24.0,
            })
            .collect(),
        markers: vec![SavedMarker {
            x: 9.25,
            y: 41.5,
            font_size: 16.0,
        }],
    };
    obj(vec![
        ("type", FigValue::Str("TEXT".into())),
        ("name", FigValue::Str("Notes".into())),
        (
            "size",
            obj(vec![
                ("x", FigValue::Float(160.0)),
                ("y", FigValue::Float(72.0)),
            ]),
        ),
        ("opacity", FigValue::Float(0.5)),
        ("fontSize", FigValue::Float(16.0)),
        (
            "fontName",
            obj(vec![
                ("family", FigValue::Str("Example Sans".into())),
                ("style", FigValue::Str("Regular".into())),
            ]),
        ),
        (
            "lineHeight",
            obj(vec![
                ("value", FigValue::Float(24.0)),
                ("units", FigValue::Str("PIXELS".into())),
            ]),
        ),
        (
            "textData",
            obj(vec![
                ("characters", FigValue::Str(characters.into())),
                (
                    "characterStyleIDs",
                    FigValue::Array(vec![FigValue::Uint(7); 19]),
                ),
                (
                    "styleOverrideTable",
                    FigValue::Array(vec![obj(vec![
                        ("styleID", FigValue::Uint(7)),
                        ("textDecoration", FigValue::Str("UNDERLINE".into())),
                    ])]),
                ),
                (
                    "lines",
                    FigValue::Array(vec![
                        obj(vec![("lineType", FigValue::Str("PLAIN".into()))]),
                        obj(vec![
                            ("lineType", FigValue::Str("UNORDERED_LIST".into())),
                            ("indentationLevel", FigValue::Uint(1)),
                        ]),
                    ]),
                ),
            ]),
        ),
        (
            "fillPaints",
            FigValue::Array(vec![solid_paint(0.0, 0.0, 0.0)]),
        ),
        (
            PRESERVED_TEXT_LAYOUT,
            FigValue::TextLayout(Box::new(layout)),
        ),
    ])
}

fn convert(figma: &FigValue, mode: FigLayoutMode) -> Option<PenNode> {
    let mut ctx = fresh_ctx();
    ctx.layout_mode = mode;
    convert_preserved_list(
        figma,
        "source",
        &SizingBehavior::Number(160.0),
        &SizingBehavior::Number(72.0),
        &mut ctx,
    )
}

#[test]
fn preserved_list_keeps_wrapped_indent_utf16_runs_and_one_parent_opacity() {
    let PenNode::Group(group) = convert(&fixture(), FigLayoutMode::Preserve).unwrap() else {
        panic!("expected group");
    };
    assert_eq!(group.base.id, "source");
    assert_eq!(group.base.name.as_deref(), Some("Notes"));
    assert_eq!(group.base.opacity, Some(NumberOrExpression::Number(0.5)));
    assert_eq!(group.container.width, Some(SizingBehavior::Number(160.0)));
    assert_eq!(group.container.height, Some(SizingBehavior::Number(72.0)));
    let children = group.children.unwrap();
    assert_eq!(children.len(), 4);
    let mut recovered = String::new();
    let mut markers = 0;
    for child in children {
        let PenNode::Text(text) = child else {
            panic!("editable text required");
        };
        assert!(
            text.base.opacity.is_none(),
            "parent opacity is applied once"
        );
        assert!(text.fill.is_some());
        if text.base.name.as_deref() == Some("Bullet") {
            markers += 1;
            assert_eq!(text.content, TextContent::Plain("•".into()));
            assert_eq!(text.base.x, Some(9.25));
            assert_eq!(text.base.y, Some(24.0));
            assert_eq!(text.underline, None);
        } else {
            let TextContent::Styled(segments) = text.content else {
                panic!("sparse styleID7 must survive slicing");
            };
            assert!(segments
                .iter()
                .all(|segment| segment.underline == Some(true)));
            recovered.extend(segments.into_iter().map(|segment| segment.text));
            if text.base.y == Some(48.0) {
                assert_eq!(text.base.x, Some(24.0), "soft wrap has hanging indent");
            }
        }
    }
    assert_eq!(markers, 1, "soft wraps must not gain a new bullet");
    assert_eq!(
        recovered, "Head\nA😀 bold words",
        "original characters survive in line order"
    );
}

#[test]
fn preserved_list_uses_saved_nested_indent_and_does_not_change_reflow_mode() {
    let mut figma = fixture();
    let FigValue::TextLayout(layout) = figma.get_mut(PRESERVED_TEXT_LAYOUT).unwrap() else {
        panic!();
    };
    layout.baselines[1].x = 48.0;
    layout.baselines[2].x = 48.0;
    layout.markers[0].x = 33.25;
    let PenNode::Group(group) = convert(&figma, FigLayoutMode::Preserve).unwrap() else {
        panic!();
    };
    assert!(group
        .children
        .unwrap()
        .iter()
        .any(|node| matches!(node, PenNode::Text(text) if text.base.x == Some(48.0))));
    assert!(
        convert(&figma, FigLayoutMode::OpenPencil).is_none(),
        "saved geometry belongs only to Preserve mode"
    );
}

#[test]
fn preserved_list_rejects_stale_ranges_resizes_and_surrogate_splits() {
    for change in 0..4 {
        let mut figma = fixture();
        let FigValue::TextLayout(layout) = figma.get_mut(PRESERVED_TEXT_LAYOUT).unwrap() else {
            panic!();
        };
        match change {
            0 => layout.baselines[1].first = 6,
            1 => layout.source_size = Some((200.0, 72.0)),
            2 => {
                layout.baselines[1].end = 7;
                layout.baselines[2].first = 7;
            }
            _ => layout.markers.clear(),
        }
        assert!(convert(&figma, FigLayoutMode::Preserve).is_none());
    }
}
