use super::*;
use crate::text_mapper::map_figma_text_props;

fn object(fields: Vec<(&str, FigValue)>) -> FigValue {
    FigValue::Object(
        fields
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
    )
}

fn number(value: f32) -> FigValue {
    FigValue::Float(value)
}
fn string(value: &str) -> FigValue {
    FigValue::Str(value.into())
}
fn guid() -> FigValue {
    object(vec![
        ("sessionID", FigValue::Uint(1)),
        ("localID", FigValue::Uint(2)),
    ])
}
fn font(family: &str) -> FigValue {
    object(vec![("family", string(family)), ("style", string("Bold"))])
}
fn pixels(value: f32) -> FigValue {
    object(vec![("value", number(value)), ("units", string("PIXELS"))])
}

fn linked_style() -> FigValue {
    object(vec![
        ("guid", guid()),
        ("styleType", string("TEXT")),
        ("fontName", font("Linked Font")),
        ("fontSize", number(20.0)),
        ("lineHeight", pixels(24.0)),
        ("letterSpacing", pixels(-1.0)),
    ])
}

fn cached_text(family: &str, tracking: f32) -> FigValue {
    let glyphs = (0..3)
        .map(|index| {
            object(vec![
                (
                    "position",
                    object(vec![
                        ("x", number(index as f32 * (10.0 + tracking))),
                        ("y", number(18.0)),
                    ]),
                ),
                ("fontSize", number(20.0)),
                ("advance", number(0.5)),
                ("firstCharacter", FigValue::Uint(index)),
                ("rotation", number(0.0)),
            ])
        })
        .collect();
    object(vec![
        ("glyphs", FigValue::Array(glyphs)),
        (
            "fontMetaData",
            FigValue::Array(vec![object(vec![("key", font(family))])].into()),
        ),
        (
            "baselines",
            FigValue::Array(vec![object(vec![("lineHeight", number(24.0))])].into()),
        ),
    ])
}

fn node(cached_family: &str, cached_tracking: f32) -> FigValue {
    object(vec![
        ("type", string("TEXT")),
        ("styleIdForText", object(vec![("guid", guid())])),
        ("fontName", font("Local Font")),
        ("fontSize", number(10.0)),
        ("lineHeight", pixels(12.0)),
        (
            "letterSpacing",
            object(vec![("value", number(0.0)), ("units", string("PERCENT"))]),
        ),
        (
            "derivedTextData",
            cached_text(cached_family, cached_tracking),
        ),
    ])
}

#[test]
fn linked_text_style_restores_family_and_tracking_confirmed_by_saved_layout() {
    let mut changes = vec![linked_style(), node("Linked Font", -1.0)];
    resolve_style_references(&mut changes);
    let props = map_figma_text_props(&changes[1]);
    assert_eq!(props.font_family.as_deref(), Some("Linked Font"));
    assert_eq!(props.font_size, Some(20.0));
    assert_eq!(props.letter_spacing, Some(-1.0));
    assert!(
        changes[1].get("derivedTextData").is_none(),
        "layout cache remains transient"
    );
}

#[test]
fn linked_text_style_preserves_confirmed_local_family_and_tracking() {
    let mut changes = vec![linked_style(), node("Local Font", 0.0)];
    resolve_style_references(&mut changes);
    let props = map_figma_text_props(&changes[1]);
    assert_eq!(props.font_family.as_deref(), Some("Local Font"));
    assert_eq!(
        props.letter_spacing, None,
        "explicit zero tracking is retained"
    );
}

#[test]
fn linked_text_style_does_not_replace_an_independent_tracking_override() {
    let mut changes = vec![linked_style(), node("Linked Font", 0.0)];
    resolve_style_references(&mut changes);
    let props = map_figma_text_props(&changes[1]);
    assert_eq!(props.font_family.as_deref(), Some("Linked Font"));
    assert_eq!(props.letter_spacing, None);
}

#[test]
fn linked_text_style_preserves_explicit_node_override_marker() {
    let mut local = node("Linked Font", -1.0);
    local.set("isOverrideOverTextStyle", FigValue::Bool(true));
    let mut changes = vec![linked_style(), local];
    resolve_style_references(&mut changes);
    let props = map_figma_text_props(&changes[1]);
    assert_eq!(props.font_family.as_deref(), Some("Local Font"));
    assert_eq!(props.letter_spacing, None);
}

#[test]
fn linked_text_style_preserves_explicit_instance_fields() {
    let instance = object(vec![(
        "symbolData",
        object(vec![(
            "symbolOverrides",
            FigValue::Array(vec![node("Linked Font", -1.0)].into()),
        )]),
    )]);
    let mut changes = vec![linked_style(), instance];
    resolve_style_references(&mut changes);
    let resolved = &changes[1]
        .get("symbolData")
        .unwrap()
        .get_array("symbolOverrides")
        .unwrap()[0];
    let props = map_figma_text_props(resolved);
    assert_eq!(props.font_family.as_deref(), Some("Local Font"));
    assert_eq!(props.font_size, Some(10.0));
    assert_eq!(props.letter_spacing, None);
}

#[test]
fn linked_text_style_percent_tracking_uses_locally_overridden_font_size() {
    let mut style = linked_style();
    style.set(
        "letterSpacing",
        object(vec![("value", number(-5.0)), ("units", string("PERCENT"))]),
    );
    let mut local = node("Linked Font", -2.0);
    local.set("fontSize", number(40.0));
    for (index, glyph) in local
        .get_mut("derivedTextData")
        .unwrap()
        .get_array_mut("glyphs")
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        glyph.set("fontSize", number(40.0));
        glyph
            .get_mut("position")
            .unwrap()
            .set("x", number(index as f32 * 18.0));
    }
    let mut changes = vec![style, local];
    resolve_style_references(&mut changes);
    let props = map_figma_text_props(&changes[1]);
    assert_eq!(
        props.font_size,
        Some(40.0),
        "preserve the local size override"
    );
    assert_eq!(
        props.letter_spacing,
        Some(-2.0),
        "percentage tracking follows the effective size"
    );
}

#[test]
fn typed_evidence_and_full_cache_resolve_identical_authored_styles() {
    for (family, tracking, explicit) in [
        ("Linked Font", -1.0, false),
        ("Local Font", 0.0, false),
        ("Linked Font", 0.0, false),
        ("Linked Font", -1.0, true),
    ] {
        let mut full = vec![linked_style(), node(family, tracking)];
        full[1].set("isOverrideOverTextStyle", FigValue::Bool(explicit));
        let mut summarized = full.clone();
        let evidence = TextEvidence::from_cache(full[1].get("derivedTextData").unwrap());
        summarized[1].set(
            "derivedTextData",
            FigValue::TextEvidence(Box::new(evidence.into_owned())),
        );
        resolve_style_references(&mut full);
        resolve_style_references(&mut summarized);
        assert_eq!(summarized, full);
    }
}

#[test]
fn linked_kerning_off_uses_saved_family_tracking_proof_and_keeps_explicit_overrides() {
    let features =
        |names: &[&str]| FigValue::Array(names.iter().map(|name| string(name)).collect());
    for explicit in [false, true] {
        let mut style = linked_style();
        style.set("toggledOffOTFeatures", features(&["LIGA", "KERN", "CALT"]));
        let mut local = node("Linked Font", -1.0);
        local.set("toggledOffOTFeatures", features(&["LIGA", "CALT"]));
        local.set("isOverrideOverTextStyle", FigValue::Bool(explicit));
        let mut changes = vec![style, local];
        resolve_style_references(&mut changes);
        assert_eq!(
            map_figma_text_props(&changes[1]).font_kerning,
            Some(explicit)
        );
    }
    let mut style = linked_style();
    style.set("toggledOffOTFeatures", features(&["KERN"]));
    let mut local = node("Linked Font", 0.0);
    local.set("toggledOffOTFeatures", features(&[]));
    let mut changes = vec![style, local];
    resolve_style_references(&mut changes);
    assert_eq!(
        map_figma_text_props(&changes[1]).font_kerning,
        Some(true),
        "unconfirmed local feature override remains authored"
    );
}

#[test]
fn linked_line_height_uses_auto_height_geometry_when_cache_stores_natural_metrics() {
    for (height, expected) in [(24.0, 24.0), (30.0, 12.0)] {
        let mut local = node("Linked Font", -1.0);
        local.set("textAutoResize", string("HEIGHT"));
        local.set(
            "size",
            object(vec![("x", number(200.0)), ("y", number(height))]),
        );
        local
            .get_mut("derivedTextData")
            .unwrap()
            .get_array_mut("baselines")
            .unwrap()[0]
            .set("lineHeight", number(27.24));
        let mut changes = vec![linked_style(), local];
        resolve_style_references(&mut changes);
        assert_eq!(effective_line_height_px(&changes[1]), Some(expected));
        assert_eq!(changes[1].get_f64("fontSize"), Some(20.0));
    }
}

#[test]
fn linked_opentype_salt_is_inherited_without_replacing_explicit_feature_arrays() {
    let mut style = linked_style();
    style.set(
        "toggledOnOTFeatures",
        FigValue::Array(vec![string("SALT")].into()),
    );
    for explicit in [false, true] {
        let mut local = node("Local Font", 0.0);
        if explicit {
            local.set("toggledOnOTFeatures", FigValue::Array(vec![].into()));
        }
        let mut changes = vec![style.clone(), local];
        resolve_style_references(&mut changes);
        let features = map_figma_text_props(&changes[1]).font_features.unwrap();
        assert_eq!(features.get("salt"), if explicit { None } else { Some(&1) });
    }
}
