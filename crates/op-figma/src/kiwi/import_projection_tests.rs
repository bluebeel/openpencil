//! Full and projected decoding consume the same wire data; only unused caches differ.

use super::*;

#[derive(Default)]
struct Wire(Vec<u8>);

impl Wire {
    fn uint(&mut self, mut value: u32) {
        loop {
            let next = (value & 127) as u8;
            value >>= 7;
            self.0.push(next | if value == 0 { 0 } else { 128 });
            if value == 0 {
                break;
            }
        }
    }

    fn string(&mut self, value: &str) {
        self.0.extend_from_slice(value.as_bytes());
        self.0.push(0);
    }

    fn float(&mut self, value: f32) {
        if value == 0.0 {
            self.0.push(0);
        } else {
            self.0
                .extend_from_slice(&value.to_bits().rotate_left(9).to_le_bytes());
        }
    }
}

fn definition(name: &str, kind: DefKind, fields: &[(&str, &str, bool)]) -> Definition {
    Definition {
        name: name.into(),
        kind,
        fields: fields
            .iter()
            .enumerate()
            .map(|(index, (name, type_name, is_array))| Field {
                name: (*name).into(),
                type_name: (*type_name).into(),
                is_array: *is_array,
                value: index as u32 + 1,
            })
            .collect(),
    }
}

fn schema() -> Schema {
    use DefKind::*;
    Schema {
        definitions: vec![
            definition(
                "Vec2",
                Struct,
                &[("x", "float", false), ("y", "float", false)],
            ),
            definition(
                "Glyph",
                Struct,
                &[
                    ("fontSize", "float", false),
                    ("position", "Vec2", false),
                    ("commandsBlob", "uint", false),
                    ("caption", "string", false),
                ],
            ),
            definition(
                "Baseline",
                Struct,
                &[("lineHeight", "float", false), ("caption", "string", false)],
            ),
            definition(
                "FontKey",
                Struct,
                &[
                    ("family", "string", false),
                    ("style", "string", false),
                    ("postscript", "string", false),
                ],
            ),
            definition(
                "FontMetadata",
                Struct,
                &[("key", "FontKey", false), ("digest", "byte", true)],
            ),
            definition(
                "Cache",
                Message,
                &[
                    ("characters", "string", false),
                    ("glyphs", "Glyph", true),
                    ("baselines", "Baseline", true),
                    ("fontMetaData", "FontMetadata", true),
                    ("bytes", "byte", true),
                    ("labels", "string", true),
                    ("child", "Cache", false),
                ],
            ),
            definition(
                "Entry",
                Message,
                &[
                    ("derivedTextData", "Cache", false),
                    ("legacyDerivedTextData", "Cache", false),
                ],
            ),
            definition("SymbolData", Message, &[("symbolOverrides", "Entry", true)]),
            definition(
                "NodeChange",
                Message,
                &[
                    ("derivedTextData", "Cache", false),
                    ("legacyDerivedTextData", "Cache", false),
                    ("symbolData", "SymbolData", false),
                    ("derivedSymbolData", "Entry", true),
                ],
            ),
            definition(
                "Message",
                Message,
                &[
                    ("nodeChanges", "NodeChange", true),
                    ("sentinel", "uint", false),
                ],
            ),
        ],
    }
}

fn cache(wire: &mut Wire, characters: Option<&str>) {
    if let Some(text) = characters {
        wire.uint(1);
        wire.string(text);
    }
    wire.uint(2);
    wire.uint(1);
    wire.float(72.0);
    wire.float(1.0);
    wire.float(2.0);
    wire.uint(99);
    wire.string("unused glyph metadata");
    wire.uint(3);
    wire.uint(1);
    wire.float(92.0);
    wire.string("unused baseline metadata");
    wire.uint(4);
    wire.uint(1);
    wire.string("Inter");
    wire.string("Bold");
    wire.string("unused postscript");
    wire.uint(2);
    wire.0.extend_from_slice(&[4, 5]);
    wire.uint(5);
    wire.uint(3);
    wire.0.extend_from_slice(&[1, 2, 3]);
    wire.uint(6);
    wire.uint(2);
    wire.string("layout");
    wire.string("cache");
    wire.uint(0);
}

fn node_prefix() -> Wire {
    let mut wire = Wire::default();
    wire.uint(1);
    wire.uint(1); // Root nodeChanges array.
    wire
}

fn first_node(root: &FigValue) -> &FigValue {
    &root.get_array("nodeChanges").unwrap()[0]
}

#[test]
fn projected_decode_preserves_direct_evidence_and_matches_existing_compaction() {
    let mut wire = node_prefix();
    wire.uint(1);
    cache(&mut wire, Some("direct"));
    wire.uint(2);
    cache(&mut wire, Some("legacy"));
    wire.uint(3);
    wire.uint(1);
    wire.uint(2); // symbolData.symbolOverrides[2].
    wire.uint(1);
    cache(&mut wire, Some("nested text"));
    wire.uint(2);
    cache(&mut wire, Some("nested legacy"));
    wire.uint(0);
    wire.uint(1);
    cache(&mut wire, None);
    wire.uint(0); // Empty marker remains.
    wire.uint(0); // symbolData.
    wire.uint(4);
    wire.uint(1); // derivedSymbolData[1].
    wire.uint(1);
    cache(&mut wire, Some("derived text"));
    wire.uint(0);
    wire.uint(0); // NodeChange.
    wire.uint(2);
    wire.uint(42);
    wire.uint(0); // Root sentinel and terminator.

    let mut full = decode_message(&schema(), &wire.0).unwrap();
    let mut projected = decode_import_message(&schema(), &wire.0).unwrap();
    assert_eq!(projected.get_f64("sentinel"), Some(42.0));
    let direct = first_node(&projected).get("derivedTextData").unwrap();
    let complete = first_node(&full).get("derivedTextData").unwrap();
    let FigValue::TextEvidence(evidence) = direct else {
        panic!("expected typed constant-space text evidence");
    };
    assert_eq!(
        evidence.as_ref(),
        &*crate::text_cache::evidence::TextEvidence::from_cache(complete)
    );
    assert_eq!(evidence.metric("glyphs", "fontSize"), Some(72.0));
    assert_eq!(evidence.metric("baselines", "lineHeight"), Some(92.0));
    assert!(complete.get("bytes").is_some());
    assert!(
        first_node(&full).get("legacyDerivedTextData").is_some(),
        "generic decoding stays complete"
    );
    assert!(first_node(&projected)
        .get("legacyDerivedTextData")
        .is_none());
    let overrides = first_node(&projected)
        .get("symbolData")
        .unwrap()
        .get_array("symbolOverrides")
        .unwrap();
    let nested = overrides[0].get("derivedTextData").unwrap();
    assert_eq!(nested.get_str("characters"), Some("nested text"));
    assert!(nested.get("glyphs").is_none());
    assert!(overrides[0].get("legacyDerivedTextData").is_none());
    assert!(overrides[1]
        .get("derivedTextData")
        .is_some_and(FigValue::is_object));

    // After the existing import compactor runs, both representations are equal.
    crate::text_cache::compact_transient_text_caches(full.get_array_mut("nodeChanges").unwrap());
    crate::text_cache::compact_transient_text_caches(
        projected.get_array_mut("nodeChanges").unwrap(),
    );
    assert_eq!(projected, full);
}

#[test]
fn skipped_cache_still_rejects_unknown_message_fields() {
    let mut wire = node_prefix();
    wire.uint(2);
    wire.uint(99);
    assert!(matches!(
        decode_import_message(&schema(), &wire.0),
        Err(KiwiError::UnknownField(_))
    ));
}

#[test]
fn direct_evidence_projection_preserves_entry_order_and_missing_fields() {
    let mut schema = schema();
    for definition in &mut schema.definitions {
        if matches!(
            definition.name.as_str(),
            "Glyph" | "Baseline" | "FontMetadata"
        ) {
            definition.kind = DefKind::Message;
        }
    }
    let mut wire = node_prefix();
    wire.uint(1); // Direct derivedTextData.
    wire.uint(2);
    wire.uint(2); // Two glyphs.
    wire.uint(1);
    wire.float(72.0);
    wire.uint(0);
    wire.uint(3);
    wire.uint(99);
    wire.uint(0); // No font size in second glyph.
    wire.uint(3);
    wire.uint(2); // Two baselines.
    wire.uint(1);
    wire.float(92.0);
    wire.uint(0);
    wire.uint(2);
    wire.string("unused");
    wire.uint(0); // No second lineHeight.
    wire.uint(4);
    wire.uint(2); // Two font metadata entries.
    wire.uint(1);
    wire.string("Inter");
    wire.string("Bold");
    wire.string("ignored");
    wire.uint(0);
    wire.uint(2);
    wire.uint(0);
    wire.uint(0); // No second font key.
    wire.uint(0);
    wire.uint(0);
    wire.uint(0);
    let full = decode_message(&schema, &wire.0).unwrap();
    let projected = decode_import_message(&schema, &wire.0).unwrap();
    let direct = first_node(&projected).get("derivedTextData").unwrap();
    let original = first_node(&full).get("derivedTextData").unwrap();
    let FigValue::TextEvidence(evidence) = direct else {
        panic!("expected typed text evidence");
    };
    assert_eq!(
        evidence.as_ref(),
        &*crate::text_cache::evidence::TextEvidence::from_cache(original)
    );
    assert_eq!(evidence.metric("glyphs", "fontSize"), None);
    assert_eq!(evidence.metric("baselines", "lineHeight"), None);
    assert!(!evidence.uses_font(
        original.get_array("fontMetaData").unwrap()[0]
            .get("key")
            .unwrap()
    ));
}

#[test]
fn skipped_cache_still_checks_bytes_strings_arrays_and_depth() {
    let mut bytes = node_prefix();
    bytes.uint(2);
    bytes.uint(5);
    bytes.uint(100);
    bytes.0.push(1);
    assert!(matches!(
        decode_import_message(&schema(), &bytes.0),
        Err(KiwiError::OutOfBounds)
    ));

    let mut string = node_prefix();
    string.uint(2);
    string.uint(1);
    string.0.extend_from_slice(b"no terminator");
    assert!(matches!(
        decode_import_message(&schema(), &string.0),
        Err(KiwiError::OutOfBounds)
    ));

    let mut array = node_prefix();
    array.uint(2);
    array.uint(2);
    array.uint(u32::MAX);
    assert!(matches!(
        decode_import_message(&schema(), &array.0),
        Err(KiwiError::OutOfBounds)
    ));

    let mut deep = node_prefix();
    deep.uint(2);
    for _ in 0..MAX_DEPTH + 1 {
        deep.uint(7);
    }
    deep.0
        .extend(std::iter::repeat_n(0, MAX_DEPTH as usize + 4));
    assert!(matches!(
        decode_import_message(&schema(), &deep.0),
        Err(KiwiError::TooDeep)
    ));
}

#[test]
fn import_requires_complete_consumption_without_changing_generic_decoder() {
    let data = [0, 123];
    assert!(decode_message(&schema(), &data).is_ok());
    assert!(matches!(
        decode_import_message(&schema(), &data),
        Err(KiwiError::TrailingBytes(1))
    ));
}

#[test]
fn streamed_text_arrays_repeat_with_last_field_winning_and_authored_data_stays_full() {
    let mut schema = schema();
    let node = schema
        .definitions
        .iter_mut()
        .find(|definition| definition.name == "NodeChange")
        .unwrap();
    node.fields.push(Field {
        name: "textData".into(),
        type_name: "Cache".into(),
        is_array: false,
        value: 5,
    });
    let mut wire = node_prefix();
    wire.uint(1);
    cache(&mut wire, Some("first"));
    wire.0.pop(); // Continue the same cache message with repeated fields.
    wire.uint(1);
    wire.string("last");
    wire.uint(2);
    wire.uint(1);
    wire.float(40.0);
    wire.float(0.0);
    wire.float(0.0);
    wire.uint(0);
    wire.string("last glyph");
    wire.uint(3);
    wire.uint(1);
    wire.float(48.0);
    wire.string("last baseline");
    wire.uint(4);
    wire.uint(0); // Last font metadata is empty, not the first valid Inter.
    wire.uint(0);
    wire.uint(5);
    cache(&mut wire, Some("authored"));
    wire.uint(0);
    wire.uint(0);
    let full = decode_message(&schema, &wire.0).unwrap();
    let projected = decode_import_message(&schema, &wire.0).unwrap();
    let direct = first_node(&projected).get("derivedTextData").unwrap();
    let FigValue::TextEvidence(evidence) = direct else {
        panic!("expected evidence");
    };
    assert_eq!(
        evidence.as_ref(),
        &*crate::text_cache::evidence::TextEvidence::from_cache(
            first_node(&full).get("derivedTextData").unwrap()
        )
    );
    assert_eq!(evidence.metric("glyphs", "fontSize"), Some(40.0));
    assert_eq!(evidence.metric("baselines", "lineHeight"), Some(48.0));
    assert_eq!(evidence.characters.as_deref(), Some("last"));
    assert_eq!(
        first_node(&projected).get("textData"),
        first_node(&full).get("textData")
    );
    assert!(first_node(&projected)
        .get("textData")
        .unwrap()
        .get("bytes")
        .is_some());
}

#[test]
fn projected_transient_structs_reserve_only_fields_the_consumer_keeps() {
    let schema = schema();
    let mut wire = Wire::default();
    wire.float(72.0);
    wire.float(1.0);
    wire.float(2.0);
    wire.uint(99);
    wire.string("ignored");
    let mut decoder = Decoder::new(&schema, &wire.0).unwrap();
    let value = decoder
        .decode_type(ResolvedType::Definition(1), 0, DecodeScope::GlyphEvidence)
        .unwrap();
    let FigValue::Object(fields) = value else {
        panic!("expected glyph object");
    };
    assert_eq!(fields.capacity(), 2);
    let point = &fields[1].1;
    assert!(matches!(point, FigValue::Vec2(_)));
    assert_eq!(point.get_f64("x"), Some(1.0));
    assert_eq!(point.get_f64("y"), Some(2.0));
}

#[test]
fn unused_consumption_maps_keep_presence_without_retaining_their_graph() {
    let mut schema = schema();
    for definition in &mut schema.definitions {
        if definition.name == "NodeChange" || definition.name == "Entry" {
            for name in ["parameterConsumptionMap", "variableConsumptionMap"] {
                definition.fields.push(Field {
                    name: name.into(),
                    type_name: "Cache".into(),
                    is_array: false,
                    value: definition.fields.len() as u32 + 1,
                });
            }
        }
    }
    let mut wire = node_prefix();
    wire.uint(5);
    cache(&mut wire, Some("direct map payload"));
    wire.uint(3);
    wire.uint(1);
    wire.uint(1);
    wire.uint(4);
    cache(&mut wire, Some("nested map payload"));
    wire.uint(0);
    wire.uint(0);
    wire.uint(0);
    wire.uint(2);
    wire.uint(99);
    wire.uint(0);
    let full = decode_message(&schema, &wire.0).unwrap();
    let projected = decode_import_message(&schema, &wire.0).unwrap();
    assert_eq!(projected.get_f64("sentinel"), Some(99.0));
    let node = first_node(&projected);
    assert_eq!(
        node.get("parameterConsumptionMap"),
        Some(&FigValue::Object(Vec::new()))
    );
    assert!(
        node.get("variableConsumptionMap").is_none(),
        "absent stays absent"
    );
    assert!(first_node(&full)
        .get("parameterConsumptionMap")
        .unwrap()
        .get("bytes")
        .is_some());
    let nested = &node
        .get("symbolData")
        .unwrap()
        .get_array("symbolOverrides")
        .unwrap()[0];
    assert_eq!(
        nested.get("variableConsumptionMap"),
        Some(&FigValue::Object(Vec::new()))
    );
    assert!(nested.get("parameterConsumptionMap").is_none());
    let FigValue::Object(fields) = node else {
        panic!("expected node");
    };
    assert_eq!(
        fields.capacity(),
        fields.len(),
        "sparse messages reserve actual encoded fields"
    );

    let mut invalid = node_prefix();
    invalid.uint(5);
    invalid.uint(99);
    assert!(matches!(
        decode_import_message(&schema, &invalid.0),
        Err(KiwiError::UnknownField(_))
    ));
}

#[test]
fn streamed_compact_list_lines_and_markers_match_full_decoding() {
    let mut schema = schema();
    let baseline = schema
        .definitions
        .iter_mut()
        .find(|definition| definition.name == "Baseline")
        .unwrap();
    for (name, type_name) in [
        ("position", "Vec2"),
        ("lineY", "float"),
        ("firstCharacter", "uint"),
        ("endCharacter", "uint"),
    ] {
        baseline.fields.push(Field {
            name: name.into(),
            type_name: type_name.into(),
            is_array: false,
            value: baseline.fields.len() as u32 + 1,
        });
    }
    let mut wire = node_prefix();
    wire.uint(1);
    wire.uint(1);
    wire.string("Head\nList");
    wire.uint(2);
    wire.uint(1);
    wire.float(16.0);
    wire.float(9.25);
    wire.float(41.5);
    wire.uint(7);
    wire.string("unused");
    wire.uint(3);
    wire.uint(2);
    for (x, y, line_y, first, end) in [(0.0, 17.5, 0.0, 0, 5), (24.0, 41.5, 24.0, 5, 9)] {
        wire.float(24.0);
        wire.string("unused");
        wire.float(x);
        wire.float(y);
        wire.float(line_y);
        wire.uint(first);
        wire.uint(end);
    }
    wire.uint(0);
    wire.uint(0);
    wire.uint(0);
    let full = decode_message(&schema, &wire.0).unwrap();
    let projected = decode_import_message(&schema, &wire.0).unwrap();
    let FigValue::TextEvidence(evidence) = first_node(&projected).get("derivedTextData").unwrap()
    else {
        panic!();
    };
    let original = crate::text_cache::evidence::TextEvidence::from_cache(
        first_node(&full).get("derivedTextData").unwrap(),
    );
    assert_eq!(evidence.as_ref(), original.as_ref());
    let layout = evidence.layout.as_ref().unwrap();
    assert_eq!(layout.baselines.len(), 2);
    assert_eq!(layout.markers.len(), 1);
    assert_eq!(layout.baselines[1].first, 5);
    assert_eq!(layout.baselines[1].x, 24.0);
    assert_eq!(layout.markers[0].x, 9.25);
}
