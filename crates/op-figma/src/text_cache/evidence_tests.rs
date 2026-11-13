use super::*;

fn object(fields: Vec<(&str, FigValue)>) -> FigValue {
    FigValue::Object(
        fields
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
    )
}

fn glyph(index: u32, x: f32, size: f32) -> FigValue {
    object(vec![
        ("firstCharacter", FigValue::Uint(index)),
        ("fontSize", FigValue::Float(size)),
        ("advance", FigValue::Float(0.5)),
        (
            "position",
            object(vec![("x", FigValue::Float(x)), ("y", FigValue::Float(0.0))]),
        ),
    ])
}

// Keep the previous full-array predicate as an independent compatibility
// oracle, including its missing-field defaults and eligible-pair rules.
fn original_step(left: &FigValue, right: &FigValue) -> Option<(f64, f64)> {
    if right.get_f64("firstCharacter")? != left.get_f64("firstCharacter")? + 1.0
        || !nearly_equal(left.get_f64("rotation").unwrap_or(0.0), 0.0)
        || !nearly_equal(right.get_f64("rotation").unwrap_or(0.0), 0.0)
    {
        return None;
    }
    let (left_position, right_position) = (left.get("position")?, right.get("position")?);
    if !nearly_equal(left_position.get_f64("y")?, right_position.get_f64("y")?) {
        return None;
    }
    let size = left.get_f64("fontSize")?;
    if size <= 0.0 || !nearly_equal(size, right.get_f64("fontSize")?) {
        return None;
    }
    let tracking = right_position.get_f64("x")?
        - left_position.get_f64("x")?
        - left.get_f64("advance")? * size;
    (tracking.is_finite() && size.is_finite()).then_some((tracking, size))
}

fn original_tracking(glyphs: &[FigValue], current: f64, referenced: f64) -> bool {
    if !current.is_finite() || !referenced.is_finite() || nearly_equal(current, referenced) {
        return false;
    }
    let mut confirmed = 0;
    for pair in glyphs.windows(2) {
        let Some((observed, size)) = original_step(&pair[0], &pair[1]) else {
            continue;
        };
        let tolerance = (size / 1024.0).max(0.05);
        if (observed - referenced).abs() > tolerance || (observed - current).abs() <= tolerance {
            return false;
        }
        confirmed += 1;
    }
    confirmed >= 2
}

#[test]
fn streaming_tracking_matches_full_array_for_ambiguous_and_invalid_steps() {
    let base = vec![
        glyph(0, 0.0, 20.0),
        glyph(1, 9.0, 20.0),
        glyph(2, 18.0, 20.0),
        glyph(3, 27.0, 20.0),
    ];
    let mut cases = vec![base.clone(), vec![], base[..1].to_vec(), base[..2].to_vec()];
    for field in [
        "firstCharacter",
        "fontSize",
        "advance",
        "position",
        "rotation",
    ] {
        for value in [
            FigValue::Null,
            FigValue::Float(f32::NAN),
            FigValue::Float(f32::INFINITY),
            FigValue::Float(-1.0),
        ] {
            let mut altered = base.clone();
            altered[1].set(field, value);
            cases.push(altered);
        }
    }
    let mut mixed = base.clone();
    mixed[2]
        .get_mut("position")
        .unwrap()
        .set("x", FigValue::Float(19.0));
    cases.push(mixed);
    let mut newline = base.clone();
    newline[2]
        .get_mut("position")
        .unwrap()
        .set("y", FigValue::Float(20.0));
    cases.push(newline);
    for glyphs in cases {
        let mut summary = TextEvidence::default();
        {
            let mut consume = summary.glyph_consumer();
            for glyph in &glyphs {
                consume(glyph);
            }
        }
        for current in [-2.0, -1.05, -1.0, -0.95, 0.0, 1.0, f64::NAN] {
            for referenced in [-1.5, -1.05, -1.0, -0.95, -0.5, 0.0, f64::NAN] {
                assert_eq!(
                    summary.confirms_tracking(current, referenced),
                    original_tracking(&glyphs, current, referenced),
                    "{glyphs:?}, current={current}, referenced={referenced}"
                );
            }
        }
    }
}

#[test]
fn text_summary_keeps_missing_mixed_and_nonfinite_metrics_invalid() {
    for values in [
        vec![],
        vec![Some(20.0), None],
        vec![None, Some(20.0)],
        vec![Some(f64::NAN)],
        vec![Some(f64::INFINITY)],
        vec![Some(20.0), Some(21.0)],
    ] {
        let mut metric = UniformMetric::default();
        for value in values {
            metric.observe(value);
        }
        assert_eq!(metric.value(), None);
    }
    let mut metric = UniformMetric::default();
    metric.observe(Some(20.0));
    metric.observe(Some(20.0009));
    assert_eq!(metric.value(), Some(20.0));
}

#[test]
fn text_summary_requires_nonempty_complete_case_insensitive_font_metadata() {
    let font = object(vec![
        ("family", FigValue::Str("Inter".into())),
        ("style", FigValue::Str("Bold".into())),
    ]);
    let mut summary = TextEvidence::default();
    assert!(!summary.uses_font(&font));
    summary.observe_font(&object(vec![("key", font.clone())]));
    assert!(summary.uses_font(&font));
    let lower = object(vec![
        ("family", FigValue::Str("inter".into())),
        ("style", FigValue::Str("bold".into())),
    ]);
    summary.observe_font(&object(vec![("key", lower)]));
    assert!(summary.uses_font(&font));
    summary.observe_font(&object(vec![]));
    assert!(!summary.uses_font(&font));
    summary.reset("fontMetaData");
    assert!(!summary.uses_font(&font));
}

#[test]
fn glyph_count_does_not_increase_retained_summary_or_figvalue_size() {
    let mut summary = TextEvidence::default();
    {
        let mut consume = summary.glyph_consumer();
        for index in 0..100_000 {
            consume(&glyph(index, index as f32 * 9.0, 20.0));
        }
    }
    assert_eq!(summary.glyph_size.count, 100_000);
    assert_eq!(summary.tracking.count, 99_999);
    assert!(std::mem::size_of::<TextEvidence>() <= 256);
    assert_eq!(
        std::mem::size_of::<FigValue>(),
        std::mem::size_of::<Vec<FigValue>>() + std::mem::size_of::<usize>()
    );
}
