//! Text-mapper tests.

use super::*;

fn obj(pairs: Vec<(&str, FigValue)>) -> FigValue {
    FigValue::Object(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
}

#[test]
fn plain_text_when_no_style_runs() {
    let node = obj(vec![(
        "textData",
        obj(vec![("characters", FigValue::Str("Hello".into()))]),
    )]);
    let props = map_figma_text_props(&node);
    assert_eq!(props.content, TextContent::Plain("Hello".into()));
    assert_eq!(props.text_align, Some(TextAlign::Left));
}

#[test]
fn kerning_maps_figma_default_enabled_and_explicit_disabled_features() {
    let features = |names: &[&str]| {
        FigValue::Array(
            names
                .iter()
                .map(|name| FigValue::Str((*name).into()))
                .collect(),
        )
    };
    for (off, on, expected) in [
        (None, None, Some(true)),
        (Some(vec![]), None, Some(true)),
        (Some(vec!["LIGA", "CALT"]), None, Some(true)),
        (Some(vec!["KERN"]), None, Some(false)),
        (None, Some(vec!["KERN"]), Some(true)),
        (Some(vec!["KERN"]), Some(vec!["KERN"]), Some(false)),
    ] {
        let mut node = obj(vec![]);
        if let Some(off) = off {
            node.set("toggledOffOTFeatures", features(&off));
        }
        if let Some(on) = on {
            node.set("toggledOnOTFeatures", features(&on));
        }
        assert_eq!(map_figma_text_props(&node).font_kerning, expected);
    }
}

#[test]
fn opentype_features_keep_salt_and_off_precedence_with_normalized_tags() {
    let mut node = obj(vec![
        (
            "toggledOnOTFeatures",
            FigValue::Array(
                ["SALT", "KERN", "SS01", "bad"]
                    .into_iter()
                    .map(|tag| FigValue::Str(tag.into()))
                    .collect(),
            ),
        ),
        (
            "toggledOffOTFeatures",
            FigValue::Array(
                ["KERN", "LIGA"]
                    .into_iter()
                    .map(|tag| FigValue::Str(tag.into()))
                    .collect(),
            ),
        ),
    ]);
    assert_eq!(
        map_figma_text_props(&node).font_features,
        Some(BTreeMap::from([
            ("kern".into(), 0),
            ("liga".into(), 0),
            ("salt".into(), 1),
            ("ss01".into(), 1),
        ]))
    );
    assert_eq!(map_figma_text_props(&node).font_kerning, Some(false));
    node.set(
        "toggledOffOTFeatures",
        FigValue::Array(vec![FigValue::Str("SALT".into())]),
    );
    assert_eq!(
        map_figma_text_props(&node)
            .font_features
            .unwrap()
            .get("salt"),
        Some(&0)
    );
    node.set("toggledOnOTFeatures", FigValue::Array(vec![]));
    node.set("toggledOffOTFeatures", FigValue::Array(vec![]));
    assert_eq!(
        map_figma_text_props(&node).font_features,
        Some(BTreeMap::new())
    );
    assert_eq!(map_figma_text_props(&obj(vec![])).font_features, None);
}

#[test]
fn figma_family_with_css_separators_is_quoted_for_the_canonical_stack() {
    assert_eq!(canonical_font_family_value("Inter"), "Inter");
    for family in ["ACME, Display", "ACME \\\" Display"] {
        let value = canonical_font_family_value(family);
        assert_eq!(
            op_editor_core::font_catalog::split_font_family_stack(&value),
            vec![family]
        );
    }
}

#[test]
fn font_weight_parsed_from_style_name() {
    assert_eq!(parse_font_weight("semibold italic"), Some(600));
    // "extra light" contains "light" → 300 (no "extralight" token).
    assert_eq!(parse_font_weight("extra light"), Some(300));
    assert_eq!(parse_font_weight("extralight"), Some(200));
    assert_eq!(parse_font_weight("extrabold"), Some(800));
    assert_eq!(parse_font_weight("bold"), Some(700));
    assert_eq!(parse_font_weight("regular"), Some(400));
}

#[test]
fn font_props_and_italic_detected() {
    let node = obj(vec![
        (
            "fontName",
            obj(vec![
                ("family", FigValue::Str("Inter".into())),
                ("style", FigValue::Str("Bold Italic".into())),
            ]),
        ),
        ("fontSize", FigValue::Float(18.0)),
        (
            "textData",
            obj(vec![("characters", FigValue::Str("x".into()))]),
        ),
    ]);
    let props = map_figma_text_props(&node);
    assert_eq!(props.font_family.as_deref(), Some("Inter"));
    assert_eq!(props.font_size, Some(18.0));
    assert_eq!(props.font_weight, Some(700));
    assert_eq!(props.font_style, Some(FontStyleKind::Italic));
}

#[test]
fn line_height_pixels_becomes_multiplier() {
    let node = obj(vec![
        ("fontSize", FigValue::Float(20.0)),
        (
            "lineHeight",
            obj(vec![
                ("units", FigValue::Str("PIXELS".into())),
                ("value", FigValue::Float(30.0)),
            ]),
        ),
    ]);
    // 30 / 20 = 1.5.
    assert_eq!(map_line_height(&node), Some(1.5));
}

#[test]
fn text_case_upper_applies() {
    let node = obj(vec![
        ("textCase", FigValue::Str("UPPER".into())),
        (
            "textData",
            obj(vec![("characters", FigValue::Str("hello".into()))]),
        ),
    ]);
    assert_eq!(
        map_figma_text_props(&node).content,
        TextContent::Plain("HELLO".into())
    );
}

#[test]
fn title_case_capitalizes_word_starts() {
    assert_eq!(title_case("hello world-foo"), "Hello World-Foo");
}

#[test]
fn align_and_growth_mapped() {
    let node = obj(vec![
        ("textAlignHorizontal", FigValue::Str("CENTER".into())),
        ("textAlignVertical", FigValue::Str("CENTER".into())),
        ("textAutoResize", FigValue::Str("HEIGHT".into())),
        ("textDecoration", FigValue::Str("UNDERLINE".into())),
    ]);
    let props = map_figma_text_props(&node);
    assert_eq!(props.text_align, Some(TextAlign::Center));
    assert_eq!(props.text_align_vertical, Some(TextAlignVertical::Middle));
    assert_eq!(props.text_growth, Some(TextGrowth::FixedWidth));
    assert_eq!(props.underline, Some(true));
}

#[test]
fn styled_segments_built_from_style_runs() {
    // 4 chars: first 2 style 0, last 2 style 1 (bold override).
    let node = obj(vec![(
        "textData",
        obj(vec![
            ("characters", FigValue::Str("abcd".into())),
            (
                "characterStyleIDs",
                FigValue::Array(vec![
                    FigValue::Int(0),
                    FigValue::Int(0),
                    FigValue::Int(1),
                    FigValue::Int(1),
                ]),
            ),
            (
                "styleOverrideTable",
                FigValue::Array(vec![
                    obj(vec![]),
                    obj(vec![(
                        "fontName",
                        obj(vec![("style", FigValue::Str("Bold".into()))]),
                    )]),
                ]),
            ),
        ]),
    )]);
    match map_figma_text_props(&node).content {
        TextContent::Styled(segs) => {
            assert_eq!(segs.len(), 2);
            assert_eq!(segs[0].text, "ab");
            assert_eq!(segs[1].text, "cd");
            assert_eq!(segs[1].font_weight, Some(700));
        }
        TextContent::Plain(p) => panic!("expected styled, got plain {p:?}"),
    }
}

fn text_with_run_override(override_style: FigValue) -> TextContent {
    let node = obj(vec![(
        "textData",
        obj(vec![
            ("characters", FigValue::Str("ab".into())),
            (
                "characterStyleIDs",
                FigValue::Array(vec![FigValue::Int(0), FigValue::Int(1)]),
            ),
            (
                "styleOverrideTable",
                FigValue::Array(vec![obj(vec![]), override_style]),
            ),
        ]),
    )]);
    map_figma_text_props(&node).content
}

#[test]
fn decoration_only_run_overrides_preserve_styled_content() {
    for decoration in ["UNDERLINE", "STRIKETHROUGH"] {
        let content = text_with_run_override(obj(vec![(
            "textDecoration",
            FigValue::Str(decoration.into()),
        )]));
        let TextContent::Styled(segments) = content else {
            panic!("{decoration} run must remain styled");
        };
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, "a");
        assert_eq!(segments[0].underline, None);
        assert_eq!(segments[0].strikethrough, None);
        assert_eq!(segments[1].text, "b");
        assert_eq!(
            segments[1].underline,
            (decoration == "UNDERLINE").then_some(true)
        );
        assert_eq!(
            segments[1].strikethrough,
            (decoration == "STRIKETHROUGH").then_some(true)
        );
    }
}

#[test]
fn italic_only_run_override_preserves_styled_content() {
    let content = text_with_run_override(obj(vec![(
        "fontName",
        obj(vec![("style", FigValue::Str("Italic".into()))]),
    )]));
    let TextContent::Styled(segments) = content else {
        panic!("italic run must remain styled");
    };
    assert_eq!(segments[0].font_style, None);
    assert_eq!(segments[1].font_style, Some(FontStyleKind::Italic));
    assert_eq!(segments[1].text, "b");
}

#[test]
fn sparse_style_ids_select_text_colors_by_declared_id() {
    let style = |id, green_blue| {
        obj(vec![
            ("styleID", FigValue::Int(id)),
            (
                "fillPaints",
                FigValue::Array(vec![obj(vec![
                    ("type", FigValue::Str("SOLID".into())),
                    (
                        "color",
                        obj(vec![
                            ("r", FigValue::Float(1.0)),
                            ("g", FigValue::Float(green_blue)),
                            ("b", FigValue::Float(green_blue)),
                            ("a", FigValue::Float(1.0)),
                        ]),
                    ),
                ])]),
            ),
        ])
    };
    let node = obj(vec![(
        "textData",
        obj(vec![
            ("characters", FigValue::Str("abcde".into())),
            (
                "characterStyleIDs",
                FigValue::Array([0, 9, 10, 1, 11].into_iter().map(FigValue::Int).collect()),
            ),
            (
                "styleOverrideTable",
                FigValue::Array(vec![style(10, 1.0), style(9, 0.0)]),
            ),
        ]),
    )]);
    let TextContent::Styled(segments) = map_figma_text_props(&node).content else {
        panic!("sparse text color overrides must remain styled");
    };
    assert_eq!(segments.len(), 5);
    assert_eq!(segments[0].fill, None);
    assert_eq!(segments[1].fill.as_deref(), Some("#ff0000"));
    assert_eq!(segments[2].fill.as_deref(), Some("#ffffff"));
    assert_eq!(segments[3].fill, None, "IDs are not array offsets");
    assert_eq!(segments[4].fill, None, "unknown IDs inherit the node style");
}
