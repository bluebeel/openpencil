//! Style-mapper tests — exercise each mapper on hand-built FigValue
//! node objects.

use super::*;

fn obj(pairs: Vec<(&str, FigValue)>) -> FigValue {
    FigValue::Object(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
}

fn color_obj(r: f64, g: f64, b: f64) -> FigValue {
    obj(vec![
        ("r", FigValue::Float(r as f32)),
        ("g", FigValue::Float(g as f32)),
        ("b", FigValue::Float(b as f32)),
    ])
}

#[test]
fn solid_fill_maps_to_hex() {
    let paints = [obj(vec![
        ("type", FigValue::Str("SOLID".into())),
        ("color", color_obj(1.0, 0.0, 0.0)),
    ])];
    let fills = map_figma_fills(Some(&paints)).expect("one fill");
    match &fills[0] {
        PenFill::Solid(b) => assert_eq!(b.color, "#ff0000"),
        _ => panic!("expected solid"),
    }
}

#[test]
fn supported_paint_blend_modes_map_to_canonical_values() {
    for (figma, expected) in [
        ("DARKEN", BlendMode::Darken),
        ("MULTIPLY", BlendMode::Multiply),
        ("SCREEN", BlendMode::Screen),
        ("OVERLAY", BlendMode::Overlay),
        ("LIGHTEN", BlendMode::Lighten),
        ("DIFFERENCE", BlendMode::Difference),
        ("HUE", BlendMode::Hue),
        ("SATURATION", BlendMode::Saturation),
        ("COLOR", BlendMode::Color),
        ("LUMINOSITY", BlendMode::Luminosity),
        ("SOFT_LIGHT", BlendMode::SoftLight),
        ("COLOR_DODGE", BlendMode::ColorDodge),
        ("COLOR_BURN", BlendMode::ColorBurn),
        ("HARD_LIGHT", BlendMode::HardLight),
        ("EXCLUSION", BlendMode::Exclusion),
    ] {
        assert_eq!(map_blend_mode(Some(figma)), Some(expected));
    }
}

#[test]
fn normal_and_unsupported_paint_blends_keep_source_over_default() {
    for figma in [
        "NORMAL",
        "PASS_THROUGH",
        "LINEAR_BURN",
        "LINEAR_DODGE",
        "UNKNOWN_FUTURE_MODE",
    ] {
        assert_eq!(map_blend_mode(Some(figma)), None);
    }
}

#[test]
fn paint_blend_is_carried_by_solid_gradient_and_image_fills() {
    let stops = || {
        FigValue::Array(
            vec![obj(vec![
                ("position", FigValue::Float(0.0)),
                ("color", color_obj(0.0, 0.0, 0.0)),
            ])]
            .into(),
        )
    };
    let paints = [
        obj(vec![
            ("type", FigValue::Str("SOLID".into())),
            ("color", color_obj(1.0, 0.0, 0.0)),
            ("blendMode", FigValue::Str("MULTIPLY".into())),
        ]),
        obj(vec![
            ("type", FigValue::Str("GRADIENT_LINEAR".into())),
            ("stops", stops()),
            ("blendMode", FigValue::Str("SCREEN".into())),
        ]),
        obj(vec![
            ("type", FigValue::Str("IMAGE".into())),
            (
                "image",
                obj(vec![("hash", FigValue::Bytes(vec![0xab, 0xcd].into()))]),
            ),
            ("blendMode", FigValue::Str("OVERLAY".into())),
        ]),
    ];
    let fills = map_figma_fills(Some(&paints)).unwrap();
    assert!(matches!(
        &fills[0],
        PenFill::Solid(body)
            if body.blend_mode.as_ref() == Some(&BlendMode::Multiply)
    ));
    assert!(matches!(
        &fills[1],
        PenFill::LinearGradient(body)
            if body.blend_mode.as_ref() == Some(&BlendMode::Screen)
    ));
    assert!(matches!(
        &fills[2],
        PenFill::Image(body)
            if body.blend_mode.as_ref() == Some(&BlendMode::Overlay)
    ));
}

#[test]
fn invisible_paints_are_retained_as_disabled() {
    let paints = [obj(vec![
        ("type", FigValue::Str("SOLID".into())),
        ("color", color_obj(0.0, 0.0, 0.0)),
        ("visible", FigValue::Bool(false)),
    ])];
    let fills = map_figma_fills(Some(&paints)).expect("retain authored paints");
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].enabled(), Some(false));
    assert!(!fills[0].is_enabled());
}

#[test]
fn linear_gradient_angle_from_transform() {
    // Direction column0 = (m00, m10) = (0, 1) → math atan2(1,0)=90°,
    // CSS angle = 90 - 90 = 0.
    let transform = obj(vec![
        ("m00", FigValue::Float(0.0)),
        ("m10", FigValue::Float(1.0)),
    ]);
    let paints = [obj(vec![
        ("type", FigValue::Str("GRADIENT_LINEAR".into())),
        (
            "stops",
            FigValue::Array(
                vec![obj(vec![
                    ("position", FigValue::Float(0.0)),
                    ("color", color_obj(0.0, 0.0, 0.0)),
                ])]
                .into(),
            ),
        ),
        ("transform", transform),
    ])];
    match &map_figma_fills(Some(&paints)).unwrap()[0] {
        PenFill::LinearGradient(g) => assert_eq!(g.angle, Some(0.0)),
        _ => panic!("expected linear gradient"),
    }
}

#[test]
fn image_fill_hash_url() {
    let paints = [obj(vec![
        ("type", FigValue::Str("IMAGE".into())),
        (
            "image",
            obj(vec![("hash", FigValue::Bytes(vec![0xab, 0xcd].into()))]),
        ),
        ("imageScaleMode", FigValue::Str("FIT".into())),
    ])];
    match &map_figma_fills(Some(&paints)).unwrap()[0] {
        PenFill::Image(img) => {
            assert_eq!(img.url, "__hash:abcd");
            assert_eq!(img.thumbnail_url, None);
            assert_eq!(img.nearest_sampling, Some(true));
            assert_eq!(img.mode, Some(ImageFillMode::Fit));
        }
        _ => panic!("expected image fill"),
    }
}

#[test]
fn image_fill_preserves_thumbnail_hash_and_nearest_sampling() {
    let paint = obj(vec![
        ("type", FigValue::Str("IMAGE".into())),
        (
            "image",
            obj(vec![("hash", FigValue::Bytes(vec![0xab, 0xcd].into()))]),
        ),
        (
            "imageThumbnail",
            obj(vec![("hash", FigValue::Bytes(vec![0x12, 0xef].into()))]),
        ),
    ]);
    let Some(PenFill::Image(image)) = map_single_fill(&paint) else {
        panic!("expected image fill");
    };
    assert_eq!(image.url, "__hash:abcd");
    assert_eq!(image.thumbnail_url.as_deref(), Some("__hash:12ef"));
    assert_eq!(image.nearest_sampling, Some(true));
}

#[test]
fn image_fill_maps_crop_and_tile_scale_modes() {
    for (figma_mode, expected) in [("CROP", ImageFillMode::Crop), ("TILE", ImageFillMode::Tile)] {
        let paints = [obj(vec![
            ("type", FigValue::Str("IMAGE".into())),
            (
                "image",
                obj(vec![("hash", FigValue::Bytes(vec![0xab, 0xcd].into()))]),
            ),
            ("imageScaleMode", FigValue::Str(figma_mode.into())),
        ])];
        match &map_figma_fills(Some(&paints)).unwrap()[0] {
            PenFill::Image(image) => assert_eq!(image.mode, Some(expected)),
            _ => panic!("expected image fill"),
        }
    }
}

#[test]
fn image_fill_maps_transformed_stretch_to_crop_and_preserves_transform() {
    let paints = [obj(vec![
        ("type", FigValue::Str("IMAGE".into())),
        (
            "image",
            obj(vec![("hash", FigValue::Bytes(vec![0xab, 0xcd].into()))]),
        ),
        ("imageScaleMode", FigValue::Str("STRETCH".into())),
        (
            "transform",
            obj(vec![
                ("m00", FigValue::Float(0.5089059)),
                ("m01", FigValue::Float(0.0)),
                ("m02", FigValue::Float(0.490246)),
                ("m10", FigValue::Float(0.0)),
                ("m11", FigValue::Float(0.28951487)),
                ("m12", FigValue::Float(0.37636933)),
            ]),
        ),
    ])];
    let fills = map_figma_fills(Some(&paints)).unwrap();
    let PenFill::Image(image) = &fills[0] else {
        panic!("expected image fill");
    };

    assert_eq!(image.mode, Some(ImageFillMode::Crop));
    let transform = image.transform.as_ref().expect("crop transform");
    assert_eq!(transform.m00, 0.5089059);
    assert_eq!(transform.m01, 0.0);
    assert_eq!(transform.m02, 0.490246);
    assert_eq!(transform.m10, 0.0);
    assert_eq!(transform.m11, 0.28951487);
    assert_eq!(transform.m12, 0.37636933);
}

#[test]
fn image_fill_keeps_untransformed_stretch_mode() {
    for transform in [
        None,
        Some(obj(vec![
            ("m00", FigValue::Float(1.0)),
            ("m11", FigValue::Float(1.0)),
        ])),
    ] {
        let mut pairs = vec![
            ("type", FigValue::Str("IMAGE".into())),
            (
                "image",
                obj(vec![("hash", FigValue::Bytes(vec![0xab, 0xcd].into()))]),
            ),
            ("imageScaleMode", FigValue::Str("STRETCH".into())),
        ];
        if let Some(transform) = transform {
            pairs.push(("transform", transform));
        }
        let paints = [obj(pairs)];
        let fills = map_figma_fills(Some(&paints)).unwrap();
        let PenFill::Image(image) = &fills[0] else {
            panic!("expected image fill");
        };

        assert_eq!(image.mode, Some(ImageFillMode::Stretch));
        assert_eq!(image.transform, None);
    }
}

#[test]
fn image_fill_maps_positive_tile_scale_only_for_tile_mode() {
    let image_fill = |mode: &str, scale: Option<f32>| {
        let mut pairs = vec![
            ("type", FigValue::Str("IMAGE".into())),
            (
                "image",
                obj(vec![("hash", FigValue::Bytes(vec![0xab, 0xcd].into()))]),
            ),
            ("imageScaleMode", FigValue::Str(mode.into())),
        ];
        if let Some(scale) = scale {
            pairs.push(("scale", FigValue::Float(scale)));
        }
        let paints = [obj(pairs)];
        let fills = map_figma_fills(Some(&paints)).unwrap();
        let PenFill::Image(image) = &fills[0] else {
            panic!("expected image fill");
        };
        image.tile_scale
    };

    assert_eq!(image_fill("TILE", Some(0.38618907)), Some(0.38618907));
    assert_eq!(image_fill("TILE", None), None);
    assert_eq!(image_fill("TILE", Some(0.0)), None);
    assert_eq!(image_fill("TILE", Some(f32::NAN)), None);
    assert_eq!(image_fill("TILE", Some(f32::INFINITY)), None);
    assert_eq!(image_fill("FIT", Some(0.38618907)), None);
}

#[test]
fn image_fill_maps_current_filter_to_slider_units() {
    let paints = [obj(vec![
        ("type", FigValue::Str("IMAGE".into())),
        (
            "image",
            obj(vec![("hash", FigValue::Bytes(vec![0xab, 0xcd].into()))]),
        ),
        (
            "paintFilter",
            obj(vec![
                ("exposure", FigValue::Float(0.5)),
                ("contrast", FigValue::Float(-0.25)),
                ("vibrance", FigValue::Float(0.75)),
                ("temperature", FigValue::Float(1.5)),
                ("tint", FigValue::Float(-1.5)),
                ("highlights", FigValue::Float(0.0)),
            ]),
        ),
    ])];
    let fills = map_figma_fills(Some(&paints)).unwrap();
    let PenFill::Image(image) = &fills[0] else {
        panic!("expected image fill");
    };
    assert_eq!(image.exposure, Some(50.0));
    assert_eq!(image.contrast, Some(-25.0));
    assert_eq!(image.saturation, Some(75.0));
    assert_eq!(image.temperature, Some(100.0));
    assert_eq!(image.tint, Some(-100.0));
    assert_eq!(image.highlights, None);
    assert_eq!(image.shadows, None);
}

#[test]
fn image_filter_falls_back_per_channel_to_legacy_adjustments() {
    let paints = [obj(vec![
        ("type", FigValue::Str("IMAGE".into())),
        (
            "image",
            obj(vec![("hash", FigValue::Bytes(vec![0xab, 0xcd].into()))]),
        ),
        ("paintFilter", obj(vec![("exposure", FigValue::Float(0.2))])),
        (
            "filterColorAdjust",
            obj(vec![
                ("exposure", FigValue::Float(0.9)),
                ("temperature", FigValue::Float(0.3)),
                ("vibrance", FigValue::Float(-0.4)),
                ("shadows", FigValue::Float(-0.1)),
            ]),
        ),
    ])];
    let fills = map_figma_fills(Some(&paints)).unwrap();
    let PenFill::Image(image) = &fills[0] else {
        panic!("expected image fill");
    };
    assert_eq!(image.exposure, Some(20.0));
    assert!((image.temperature.unwrap() - 30.0).abs() < 0.0001);
    assert_eq!(image.saturation, Some(-40.0));
    assert!((image.shadows.unwrap() + 10.0).abs() < 0.0001);
}

#[test]
fn stroke_uniform_thickness() {
    let node = obj(vec![
        (
            "strokePaints",
            FigValue::Array(
                vec![obj(vec![
                    ("type", FigValue::Str("SOLID".into())),
                    ("color", color_obj(0.0, 0.0, 0.0)),
                ])]
                .into(),
            ),
        ),
        ("strokeWeight", FigValue::Float(2.5)),
        ("strokeAlign", FigValue::Str("INSIDE".into())),
    ]);
    let stroke = map_figma_stroke(&node).expect("stroke present");
    assert!(matches!(stroke.thickness, StrokeThickness::Uniform(2.5)));
    assert_eq!(stroke.align, Some(StrokeAlign::Inside));
    assert!(stroke.fill.is_some());
}

#[test]
fn stroke_per_side_thickness() {
    let node = obj(vec![
        (
            "strokePaints",
            FigValue::Array(
                vec![obj(vec![
                    ("type", FigValue::Str("SOLID".into())),
                    ("color", color_obj(0.0, 0.0, 0.0)),
                ])]
                .into(),
            ),
        ),
        ("borderStrokeWeightsIndependent", FigValue::Bool(true)),
        ("borderTopWeight", FigValue::Float(1.0)),
        ("borderRightWeight", FigValue::Float(2.0)),
        ("borderBottomWeight", FigValue::Float(3.0)),
        ("borderLeftWeight", FigValue::Float(4.0)),
    ]);
    let stroke = map_figma_stroke(&node).unwrap();
    assert!(matches!(
        stroke.thickness,
        StrokeThickness::PerSide([1.0, 2.0, 3.0, 4.0])
    ));
}

#[test]
fn drop_shadow_effect() {
    let effects = [obj(vec![
        ("type", FigValue::Str("DROP_SHADOW".into())),
        (
            "offset",
            obj(vec![
                ("x", FigValue::Float(2.0)),
                ("y", FigValue::Float(4.0)),
            ]),
        ),
        ("radius", FigValue::Float(8.0)),
    ])];
    match &map_figma_effects(Some(&effects)).unwrap()[0] {
        PenEffect::Shadow(s) => {
            assert_eq!(s.inner, Some(false));
            assert_eq!(s.offset_x, 2.0);
            assert_eq!(s.blur, 8.0);
            assert_eq!(s.color, "#00000040");
        }
        _ => panic!("expected shadow"),
    }
}

#[test]
fn shadow_colors_keep_source_float_bits_through_the_scene_codec() {
    let rgb = [f32::from_bits(0x3e808180), 10.0_f32 / 255.0, 0.71234566];
    for kind in ["DROP_SHADOW", "INNER_SHADOW"] {
        for alpha in [0.0_f32, 0.05, 0.1, 0.33333334, 1.0] {
            let effects = map_figma_effects(Some(&[obj(vec![
                ("type", FigValue::Str(kind.into())),
                ("radius", FigValue::Float(10.0)),
                (
                    "color",
                    obj(vec![
                        ("r", FigValue::Float(rgb[0])),
                        ("g", FigValue::Float(rgb[1])),
                        ("b", FigValue::Float(rgb[2])),
                        ("a", FigValue::Float(alpha)),
                    ]),
                ),
            ])]))
            .unwrap();
            let source = serde_json::json!({"version":"1.0.0","children":[{
                "type":"rectangle","id":"shadow","width":10,"height":10,
                "effects":effects}]})
            .to_string();
            let doc = jian_ops_schema::load_str(&source).unwrap().value;
            let scene = op_pen_loader::pen_document_to_layout_scene_with_geometry_mode(
                &doc,
                &std::collections::BTreeMap::new(),
                0,
                true,
            );
            let effect = &scene.active_page().unwrap().children[0].effects[0];
            let op_editor_ui::layout_scene::Effect::DropShadow(shadow) = effect else {
                panic!("scene shadow");
            };
            assert_eq!(
                [
                    shadow.color.r,
                    shadow.color.g,
                    shadow.color.b,
                    shadow.color.a
                ]
                .map(f32::to_bits),
                [rgb[0], rgb[1], rgb[2], alpha].map(f32::to_bits)
            );
            assert_eq!(shadow.inner, kind == "INNER_SHADOW");
        }
    }
}

#[test]
fn shadow_float_codec_keeps_defaults_clamping_and_ordinary_solid_hex() {
    assert_eq!(
        shadow_color_css(FigColor {
            r: 1.2,
            g: -0.2,
            b: 0.0,
            a: None
        }),
        "rgba(255,0,0,1)"
    );
    assert_eq!(
        shadow_color_css(FigColor {
            r: f64::NAN,
            g: 1.0,
            b: 0.0,
            a: None
        }),
        "#00ff00"
    );
    let color = obj(vec![
        ("r", FigValue::Float(0.25)),
        ("g", FigValue::Float(0.5)),
        ("b", FigValue::Float(0.125)),
        ("a", FigValue::Float(0.05)),
    ]);
    let fill = map_single_fill(&obj(vec![
        ("type", FigValue::Str("SOLID".into())),
        ("color", color),
    ]));
    let Some(PenFill::Solid(fill)) = fill else {
        panic!("solid fill");
    };
    assert_eq!(fill.color, "#4080200d");
}

#[test]
fn background_blur_effect() {
    let effects = [obj(vec![
        ("type", FigValue::Str("BACKGROUND_BLUR".into())),
        ("radius", FigValue::Float(12.0)),
    ])];
    match &map_figma_effects(Some(&effects)).unwrap()[0] {
        PenEffect::BackgroundBlur(b) => assert_eq!(b.radius, 12.0),
        _ => panic!("expected background blur"),
    }
}

#[test]
fn layout_horizontal_with_gap_and_padding() {
    let node = obj(vec![
        ("stackMode", FigValue::Str("HORIZONTAL".into())),
        ("stackSpacing", FigValue::Float(12.0)),
        ("stackPadding", FigValue::Float(8.0)),
        ("stackPrimaryAlignItems", FigValue::Str("CENTER".into())),
        ("stackCounterAlignItems", FigValue::Str("MIN".into())),
    ]);
    let l = map_figma_layout(&node);
    assert_eq!(l.layout, Some(LayoutMode::Horizontal));
    assert_eq!(l.gap, Some(12.0));
    assert_eq!(l.padding, Some(Padding::Uniform(8.0)));
    assert_eq!(l.justify_content, Some(JustifyContent::Center));
    assert_eq!(l.align_items, Some(AlignItems::Start));
    assert_eq!(l.clip_content, Some(true));
}

#[test]
fn layout_space_between_skips_gap() {
    let node = obj(vec![
        ("stackMode", FigValue::Str("VERTICAL".into())),
        ("stackSpacing", FigValue::Float(10.0)),
        (
            "stackPrimaryAlignItems",
            FigValue::Str("SPACE_EVENLY".into()),
        ),
    ]);
    let l = map_figma_layout(&node);
    assert_eq!(l.justify_content, Some(JustifyContent::SpaceBetween));
    assert_eq!(l.gap, None);
}

#[test]
fn layout_preserves_disabled_frame_mask_as_explicit_open_content() {
    let node = obj(vec![("frameMaskDisabled", FigValue::Bool(true))]);
    assert_eq!(map_figma_layout(&node).clip_content, Some(false));
}

#[test]
fn padding_per_side_quad() {
    let node = obj(vec![
        ("stackVerticalPadding", FigValue::Float(4.0)),
        ("stackHorizontalPadding", FigValue::Float(8.0)),
        ("stackPaddingBottom", FigValue::Float(16.0)),
    ]);
    // top=4, right=8, bottom=16, left=8 → not all-equal, not v==v/h==h.
    assert_eq!(
        map_padding(&node),
        Some(Padding::LtrB([4.0, 8.0, 16.0, 8.0]))
    );
}

#[test]
fn width_sizing_fill_container_in_horizontal_parent() {
    let node = obj(vec![("stackChildPrimaryGrow", FigValue::Int(1))]);
    assert!(matches!(
        map_width_sizing(&node, Some("HORIZONTAL")),
        SizingBehavior::Keyword(SizingKeyword::FillContainer)
    ));
}

#[test]
fn width_sizing_falls_back_to_size_x() {
    let node = obj(vec![(
        "size",
        obj(vec![
            ("x", FigValue::Float(240.0)),
            ("y", FigValue::Float(80.0)),
        ]),
    )]);
    assert!(matches!(
        map_width_sizing(&node, None),
        SizingBehavior::Number(n) if n == 240.0
    ));
}

#[test]
fn height_sizing_fit_content_in_vertical_stack() {
    let node = obj(vec![
        ("stackMode", FigValue::Str("VERTICAL".into())),
        ("stackPrimarySizing", FigValue::Str("RESIZE_TO_FIT".into())),
    ]);
    assert!(matches!(
        map_height_sizing(&node, None),
        SizingBehavior::Keyword(SizingKeyword::FitContent)
    ));
}

#[test]
fn image_fill_rotation_retains_degrees_and_full_turns_without_nonfinite_values() {
    for rotation in [0.0, 90.0, 360.0, -45.0] {
        let paint = obj(vec![
            ("type", FigValue::Str("IMAGE".into())),
            ("rotation", FigValue::Float(rotation)),
        ]);
        let Some(PenFill::Image(image)) = map_single_fill(&paint) else {
            panic!()
        };
        assert_eq!(image.rotation, Some(rotation as f64));
    }
    for rotation in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let paint = obj(vec![
            ("type", FigValue::Str("IMAGE".into())),
            ("rotation", FigValue::Float(rotation)),
        ]);
        let Some(PenFill::Image(image)) = map_single_fill(&paint) else {
            panic!()
        };
        assert_eq!(image.rotation, None);
    }
    let Some(PenFill::Image(image)) =
        map_single_fill(&obj(vec![("type", FigValue::Str("IMAGE".into()))]))
    else {
        panic!()
    };
    assert_eq!(image.rotation, None);
}

#[test]
fn triangular_shadow_profile_is_limited_to_known_uniform_rounded_geometry() {
    for (kind, expected) in [
        ("FRAME", true),
        ("RECTANGLE", true),
        ("INSTANCE", true),
        ("VECTOR", false),
        ("TEXT", false),
    ] {
        let node = obj(vec![
            ("type", FigValue::Str(kind.into())),
            (
                "effects",
                FigValue::Array(
                    vec![obj(vec![
                        ("type", FigValue::Str("DROP_SHADOW".into())),
                        ("radius", FigValue::Float(8.0)),
                    ])]
                    .into(),
                ),
            ),
        ]);
        let effects = map_figma_node_effects(&node).unwrap();
        let PenEffect::Shadow(s) = &effects[0] else {
            panic!("shadow")
        };
        assert_eq!(s.blur_kernel.is_some(), expected);
    }
    for key in ["cornerSmoothing", "rectangleCornerSmoothing"] {
        let node = obj(vec![
            ("type", FigValue::Str("FRAME".into())),
            (key, FigValue::Float(0.5)),
            (
                "effects",
                FigValue::Array(
                    vec![obj(vec![("type", FigValue::Str("DROP_SHADOW".into()))])].into(),
                ),
            ),
        ]);
        let effects = map_figma_node_effects(&node).unwrap();
        let PenEffect::Shadow(s) = &effects[0] else {
            panic!("shadow")
        };
        assert_eq!(s.blur_kernel, None);
    }
}
