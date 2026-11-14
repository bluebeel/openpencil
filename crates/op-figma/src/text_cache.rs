//! Post-style-resolution compaction for transient Figma text layout caches.

use crate::kiwi::FigValue;

#[path = "text_cache/evidence.rs"]
pub(crate) mod evidence;
#[path = "text_cache/layout.rs"]
pub(crate) mod layout;

/// Drop decoded layout caches after they have served style resolution.
///
/// Figma repeats glyph, baseline, and font metadata under both direct
/// text nodes and instance-derived entries. Conversion only needs the
/// direct cache while resolving linked text metrics. Instance matching
/// subsequently observes `derivedTextData` presence as a text-target
/// marker and optionally reads `characters`, so retain exactly that
/// compact semantic payload in nested entries.
pub(crate) fn compact_transient_text_caches(node_changes: &mut [FigValue]) {
    for node in node_changes {
        let saved_layout = layout::has_unordered_list(node)
            .then(|| node.get("derivedTextData"))
            .flatten()
            .and_then(|cache| evidence::TextEvidence::from_cache(cache).layout.clone());
        if let Some(mut saved) = saved_layout {
            saved.source_size = node
                .get("size")
                .and_then(|size| Some((size.get_f64("x")?, size.get_f64("y")?)));
            node.set(layout::PRESERVED_TEXT_LAYOUT, FigValue::TextLayout(saved));
        }
        let FigValue::Object(fields) = node else {
            continue;
        };
        for (name, value) in fields.iter_mut() {
            if matches!(name.as_ref(), "derivedSymbolData" | "symbolData") {
                compact_instance_text_caches(value);
            }
        }
        let old_len = fields.len();
        fields.retain(|(name, _)| {
            !matches!(name.as_ref(), "derivedTextData" | "legacyDerivedTextData")
        });
        if fields.len() != old_len {
            fields.shrink_to_fit();
        }
    }
}

fn compact_instance_text_caches(value: &mut FigValue) {
    match value {
        FigValue::Array(values) => {
            for value in values {
                compact_instance_text_caches(value);
            }
        }
        FigValue::Object(fields) => {
            for (name, child) in fields.iter_mut() {
                if name.as_ref() == "derivedTextData" {
                    compact_derived_text_data(child);
                } else {
                    compact_instance_text_caches(child);
                }
            }
            let old_len = fields.len();
            fields.retain(|(name, _)| name.as_ref() != "legacyDerivedTextData");
            if fields.len() != old_len {
                fields.shrink_to_fit();
            }
        }
        _ => {}
    }
}

fn compact_derived_text_data(value: &mut FigValue) {
    let FigValue::Object(fields) = value else {
        return;
    };
    fields.retain(|(name, _)| name.as_ref() == "characters");
    fields.shrink_to_fit();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(fields: Vec<(&str, FigValue)>) -> FigValue {
        FigValue::Object(
            fields
                .into_iter()
                .map(|(name, value)| (name.into(), value))
                .collect(),
        )
    }

    #[test]
    fn drops_layout_only_caches_but_keeps_instance_text_semantics() {
        let with_characters = object(vec![
            ("characters", FigValue::Str("Updated".into())),
            (
                "glyphs",
                FigValue::Array(vec![object(vec![("fontSize", FigValue::Float(16.0))])].into()),
            ),
            (
                "fontMetaData",
                FigValue::Array(
                    vec![object(vec![("family", FigValue::Str("Inter".into()))])].into(),
                ),
            ),
        ]);
        let marker = object(vec![(
            "glyphs",
            FigValue::Array(vec![object(vec![("fontSize", FigValue::Float(12.0))])].into()),
        )]);
        let mut changes = vec![object(vec![
            ("derivedTextData", marker.clone()),
            ("legacyDerivedTextData", marker.clone()),
            (
                "derivedSymbolData",
                FigValue::Array(
                    vec![
                        object(vec![
                            ("size", object(vec![("x", FigValue::Float(10.0))])),
                            ("derivedTextData", with_characters),
                            ("legacyDerivedTextData", marker.clone()),
                        ]),
                        object(vec![("derivedTextData", marker)]),
                    ]
                    .into(),
                ),
            ),
        ])];

        compact_transient_text_caches(&mut changes);

        assert!(changes[0].get("derivedTextData").is_none());
        assert!(changes[0].get("legacyDerivedTextData").is_none());
        let entries = changes[0]
            .get_array("derivedSymbolData")
            .expect("derived entries survive");
        assert_eq!(
            entries[0].get("size").and_then(|value| value.get_f64("x")),
            Some(10.0)
        );
        let compact_text = entries[0]
            .get("derivedTextData")
            .expect("text marker survives");
        assert_eq!(compact_text.get_str("characters"), Some("Updated"));
        assert!(compact_text.get("glyphs").is_none());
        assert!(compact_text.get("fontMetaData").is_none());
        assert!(entries[0].get("legacyDerivedTextData").is_none());
        assert!(entries[1]
            .get("derivedTextData")
            .is_some_and(FigValue::is_object));
    }
    #[test]
    fn only_authored_lists_keep_compact_line_geometry_after_style_resolution() {
        let mut evidence = evidence::TextEvidence::default();
        evidence.layout = Some(Box::new(layout::SavedTextLayout {
            baselines: vec![layout::SavedBaseline {
                x: 24.0,
                y: 17.5,
                line_y: 0.0,
                line_height: 24.0,
                first: 0,
                end: 4,
            }],
            markers: vec![layout::SavedMarker {
                x: 9.25,
                y: 17.5,
                font_size: 16.0,
            }],
            source_size: None,
        }));
        let make_node = |line_type: &str| {
            object(vec![
                (
                    "size",
                    object(vec![
                        ("x", FigValue::Float(100.0)),
                        ("y", FigValue::Float(24.0)),
                    ]),
                ),
                (
                    "textData",
                    object(vec![
                        ("characters", FigValue::Str("List".into())),
                        (
                            "lines",
                            FigValue::Array(
                                vec![object(vec![("lineType", FigValue::Str(line_type.into()))])]
                                    .into(),
                            ),
                        ),
                    ]),
                ),
                (
                    "derivedTextData",
                    FigValue::TextEvidence(Box::new(evidence.clone())),
                ),
            ])
        };
        let mut nodes = vec![make_node("UNORDERED_LIST"), make_node("PLAIN")];
        compact_transient_text_caches(&mut nodes);
        let FigValue::TextLayout(saved) = nodes[0].get(layout::PRESERVED_TEXT_LAYOUT).unwrap()
        else {
            panic!("typed line layout expected");
        };
        assert_eq!(saved.baselines.len(), 1);
        assert_eq!(saved.markers.len(), 1);
        assert_eq!(saved.source_size, Some((100.0, 24.0)));
        assert!(nodes[0].get("derivedTextData").is_none());
        assert!(
            nodes[1].get(layout::PRESERVED_TEXT_LAYOUT).is_none(),
            "ordinary text does not retain layout records"
        );
    }
}
