//! End-to-end binary `.fig` test — assembles a real Kiwi-encoded
//! `.fig` container from scratch (schema chunk + data chunk, both
//! deflate-compressed) and drives it through the full
//! [`parse_fig_binary`] pipeline.

use crate::{parse_fig_binary, prepare_fig_binary, FigLayoutMode, FigParseError};
use crate::{prepare_fig_binary_with_observer, FigImportPhase};
use jian_ops_schema::node::PenNode;
use jian_ops_schema::sizing::SizingBehavior;
use std::io::Write;

#[test]
fn unused_consumption_map_markers_preserve_converted_document_and_warnings() {
    use crate::figma_types::FigmaDecodedFile;
    use crate::kiwi::{decode_binary_schema, decode_message, FigValue};
    let schema = decode_binary_schema(&build_schema()).unwrap();
    let mut root = decode_message(&schema, &build_data()).unwrap();
    let mut full = root.get_array_mut("nodeChanges").unwrap().clone();
    let full_map = FigValue::Object(
        vec![(
            "entries".into(),
            FigValue::Array(
                vec![FigValue::Object(
                    vec![("unused".into(), FigValue::Str("binding bookkeeping".into()))].into(),
                )]
                .into(),
            ),
        )]
        .into(),
    );
    for node in &mut full {
        node.set("parameterConsumptionMap", full_map.clone());
        node.set("variableConsumptionMap", full_map.clone());
    }
    let mut projected = full.clone();
    for node in &mut projected {
        node.set(
            "parameterConsumptionMap",
            FigValue::Object(Vec::new().into()),
        );
        node.set(
            "variableConsumptionMap",
            FigValue::Object(Vec::new().into()),
        );
    }
    let convert = |node_changes| {
        crate::node_mapper::figma_to_pen_document(
            FigmaDecodedFile {
                node_changes,
                blobs: vec![],
                image_files: Default::default(),
            },
            "Map markers",
            0,
            FigLayoutMode::Preserve,
        )
    };
    let full = convert(full.to_vec());
    let projected = convert(projected.to_vec());
    assert_eq!(
        serde_json::to_value(projected.document).unwrap(),
        serde_json::to_value(full.document).unwrap()
    );
    assert_eq!(projected.warnings, full.warnings);
}

/// Minimal Kiwi wire writer (mirrors the in-crate decoder).
#[derive(Default)]
struct W {
    out: Vec<u8>,
}

impl W {
    fn byte(&mut self, b: u8) {
        self.out.push(b);
    }
    fn var_uint(&mut self, mut v: u32) {
        loop {
            let mut byte = (v & 127) as u8;
            v >>= 7;
            if v != 0 {
                byte |= 128;
            }
            self.out.push(byte);
            if v == 0 {
                break;
            }
        }
    }
    fn var_int(&mut self, v: i32) {
        self.var_uint(((v << 1) ^ (v >> 31)) as u32);
    }
    fn var_float(&mut self, v: f32) {
        let stored = v.to_bits().rotate_left(9);
        if stored & 0xff == 0 {
            self.out.push(0);
        } else {
            self.out.extend_from_slice(&stored.to_le_bytes());
        }
    }
    fn string(&mut self, s: &str) {
        self.out.extend_from_slice(s.as_bytes());
        self.out.push(0);
    }
    /// Schema field row: name, type code, array flag, value.
    fn field(&mut self, name: &str, type_code: i32, is_array: bool, value: u32) {
        self.string(name);
        self.var_int(type_code);
        self.byte(if is_array { 1 } else { 0 });
        self.var_uint(value);
    }
}

/// Build the fixture schema chunk. Definition indices: GUID=0, Vec=1,
/// Matrix=2, ParentIndex=3, NodeType=4, NodeChange=5, Message=6.
fn build_schema() -> Vec<u8> {
    let mut w = W::default();
    w.var_uint(7); // definition count

    // 0: struct GUID { sessionID: uint, localID: uint }
    w.string("GUID");
    w.byte(1);
    w.var_uint(2);
    w.field("sessionID", -4, false, 0);
    w.field("localID", -4, false, 0);

    // 1: struct Vec { x: float, y: float }
    w.string("Vec");
    w.byte(1);
    w.var_uint(2);
    w.field("x", -5, false, 0);
    w.field("y", -5, false, 0);

    // 2: struct Matrix { m00..m12: float }
    w.string("Matrix");
    w.byte(1);
    w.var_uint(6);
    for m in ["m00", "m01", "m02", "m10", "m11", "m12"] {
        w.field(m, -5, false, 0);
    }

    // 3: struct ParentIndex { guid: GUID, position: string }
    w.string("ParentIndex");
    w.byte(1);
    w.var_uint(2);
    w.field("guid", 0, false, 0);
    w.field("position", -6, false, 0);

    // 4: enum NodeType { DOCUMENT=1, CANVAS=2, FRAME=3, RECTANGLE=4 }
    w.string("NodeType");
    w.byte(0);
    w.var_uint(4);
    w.field("DOCUMENT", 0, false, 1);
    w.field("CANVAS", 0, false, 2);
    w.field("FRAME", 0, false, 3);
    w.field("RECTANGLE", 0, false, 4);

    // 5: message NodeChange
    w.string("NodeChange");
    w.byte(2);
    w.var_uint(6);
    w.field("guid", 0, false, 1);
    w.field("parentIndex", 3, false, 2);
    w.field("type", 4, false, 3);
    w.field("name", -6, false, 4);
    w.field("size", 1, false, 5);
    w.field("transform", 2, false, 6);

    // 6: message Message { nodeChanges: NodeChange[] }
    w.string("Message");
    w.byte(2);
    w.var_uint(1);
    w.field("nodeChanges", 5, true, 1);

    w.out
}

/// One node-change descriptor for the data chunk.
struct NodeSpec {
    local_id: u32,
    parent: Option<(u32, u32, &'static str)>, // (session, local, position)
    type_value: u32,
    name: &'static str,
    size: Option<(f32, f32)>,
    transform: Option<[f32; 6]>,
}

fn encode_guid(w: &mut W, session: u32, local: u32) {
    w.var_uint(session);
    w.var_uint(local);
}

fn encode_node(w: &mut W, n: &NodeSpec) {
    // guid — field id 1.
    w.var_uint(1);
    encode_guid(w, 0, n.local_id);
    // parentIndex — field id 2.
    if let Some((s, l, pos)) = n.parent {
        w.var_uint(2);
        encode_guid(w, s, l);
        w.string(pos);
    }
    // type — field id 3.
    w.var_uint(3);
    w.var_uint(n.type_value);
    // name — field id 4.
    w.var_uint(4);
    w.string(n.name);
    // size — field id 5.
    if let Some((x, y)) = n.size {
        w.var_uint(5);
        w.var_float(x);
        w.var_float(y);
    }
    // transform — field id 6.
    if let Some(m) = n.transform {
        w.var_uint(6);
        for v in m {
            w.var_float(v);
        }
    }
    w.var_uint(0); // end NodeChange
}

/// Build the data chunk: DOCUMENT → CANVAS → RECTANGLE.
fn build_data() -> Vec<u8> {
    let nodes = [
        NodeSpec {
            local_id: 1,
            parent: None,
            type_value: 1, // DOCUMENT
            name: "Doc",
            size: None,
            transform: None,
        },
        NodeSpec {
            local_id: 2,
            parent: Some((0, 1, "a")),
            type_value: 2, // CANVAS
            name: "Page 1",
            size: None,
            transform: None,
        },
        NodeSpec {
            local_id: 3,
            parent: Some((0, 2, "a")),
            type_value: 4, // RECTANGLE
            name: "Box",
            size: Some((100.0, 50.0)),
            transform: Some([1.0, 0.0, 10.0, 0.0, 1.0, 20.0]),
        },
    ];
    let mut w = W::default();
    w.var_uint(1); // Message field id 1 = nodeChanges
    w.var_uint(nodes.len() as u32);
    for n in &nodes {
        encode_node(&mut w, n);
    }
    w.var_uint(0); // end Message
    w.out
}

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

/// Assemble a bare `fig-kiwi` container around the two chunks.
fn build_fig(schema: &[u8], data: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"fig-kiwi");
    buf.extend_from_slice(&[0, 0, 0, 0]); // delimiter
    for chunk in [schema, data] {
        let comp = deflate(chunk);
        buf.extend_from_slice(&(comp.len() as u32).to_le_bytes());
        buf.extend_from_slice(&comp);
    }
    buf
}

#[test]
fn parses_a_full_binary_fig_into_a_document() {
    let fig = build_fig(&build_schema(), &build_data());
    let import = parse_fig_binary(&fig, "Test", FigLayoutMode::OpenPencil)
        .expect("binary .fig parses end-to-end");

    let pages = import.document.pages.expect("document has pages");
    assert_eq!(pages.len(), 1, "one user page");
    assert_eq!(pages[0].name, "Page 1");
    assert_eq!(pages[0].children.len(), 1, "one rectangle on the page");

    match &pages[0].children[0] {
        PenNode::Rectangle(rect) => {
            assert_eq!(rect.base.name.as_deref(), Some("Box"));
            // transform translation (m02, m12) → top-left.
            assert_eq!(rect.base.x, Some(10.0));
            assert_eq!(rect.base.y, Some(20.0));
            assert!(matches!(
                rect.container.width,
                Some(SizingBehavior::Number(w)) if w == 100.0
            ));
            assert!(matches!(
                rect.container.height,
                Some(SizingBehavior::Number(h)) if h == 50.0
            ));
        }
        other => panic!("expected a Rectangle, got {other:?}"),
    }
}

#[test]
fn observer_reports_phases_without_changing_preparation_or_page_conversion() {
    let fig = build_fig(&build_schema(), &build_data());
    let ordinary = prepare_fig_binary(&fig, "Test", FigLayoutMode::Preserve).unwrap();
    let mut phases = Vec::new();
    let observed =
        prepare_fig_binary_with_observer(&fig, "Test", FigLayoutMode::Preserve, &mut |phase| {
            phases.push(phase)
        })
        .unwrap();
    assert_eq!(observed.pages(), ordinary.pages());
    let observed = observed
        .into_page_with_observer(0, &mut |phase| phases.push(phase))
        .unwrap();
    let ordinary = ordinary.into_page(0).unwrap();
    assert_eq!(
        serde_json::to_value(observed.document).unwrap(),
        serde_json::to_value(ordinary.document).unwrap()
    );
    assert_eq!(observed.warnings, ordinary.warnings);
    assert_eq!(
        phases,
        [
            FigImportPhase::Container,
            FigImportPhase::Schema,
            FigImportPhase::Decode,
            FigImportPhase::Extract,
            FigImportPhase::ResolveStyles,
            FigImportPhase::BuildTree,
            FigImportPhase::Prepared,
            FigImportPhase::ConvertPage,
            FigImportPhase::ReleaseSource,
            FigImportPhase::ResolveImages,
            FigImportPhase::Complete,
        ]
    );
}

#[test]
fn prepared_binary_exposes_pages_before_converting_one() {
    let fig = build_fig(&build_schema(), &build_data());
    let prepared = prepare_fig_binary(&fig, "Prepared", FigLayoutMode::Preserve)
        .expect("binary .fig prepares");

    assert_eq!(prepared.pages().len(), 1);
    assert_eq!(prepared.pages()[0].id, "0:2");
    assert_eq!(prepared.pages()[0].name, "Page 1");
    assert_eq!(prepared.pages()[0].child_count, 1);

    let import = prepared.into_page(0).expect("first page converts");
    let pages = import.document.pages.expect("document has pages");
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].name, "Page 1");
    assert_eq!(pages[0].children.len(), 1);
}

#[test]
fn prepared_binary_rejects_an_out_of_range_page() {
    let fig = build_fig(&build_schema(), &build_data());
    let prepared = prepare_fig_binary(&fig, "Prepared", FigLayoutMode::Preserve)
        .expect("binary .fig prepares");
    let error = prepared.into_page(1).expect_err("only page zero exists");

    assert!(matches!(
        &error,
        FigParseError::PageOutOfBounds {
            index: 1,
            page_count: 1
        }
    ));
    assert_eq!(
        error.to_string(),
        "Figma page index 1 is out of bounds (page count: 1)"
    );
}

/// Wrap a payload as `canvas.fig` in a minimal stored (uncompressed)
/// ZIP archive — the common Figma export form.
fn wrap_in_zip(canvas: &[u8]) -> Vec<u8> {
    const LFH: u32 = 0x0403_4b50;
    const CDFH: u32 = 0x0201_4b50;
    const EOCD: u32 = 0x0605_4b50;
    let name = b"canvas.fig";
    let mut z = Vec::new();
    let local_off = 0u32;
    z.extend_from_slice(&LFH.to_le_bytes());
    z.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    z.extend_from_slice(&(canvas.len() as u32).to_le_bytes());
    z.extend_from_slice(&(canvas.len() as u32).to_le_bytes());
    z.extend_from_slice(&(name.len() as u16).to_le_bytes());
    z.extend_from_slice(&[0, 0]);
    z.extend_from_slice(name);
    z.extend_from_slice(canvas);
    let cd_off = z.len() as u32;
    z.extend_from_slice(&CDFH.to_le_bytes());
    z.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    z.extend_from_slice(&(canvas.len() as u32).to_le_bytes());
    z.extend_from_slice(&(canvas.len() as u32).to_le_bytes());
    z.extend_from_slice(&(name.len() as u16).to_le_bytes());
    z.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    z.extend_from_slice(&local_off.to_le_bytes());
    z.extend_from_slice(name);
    let cd_size = z.len() as u32 - cd_off;
    z.extend_from_slice(&EOCD.to_le_bytes());
    z.extend_from_slice(&[0, 0, 0, 0, 1, 0, 1, 0]);
    z.extend_from_slice(&cd_size.to_le_bytes());
    z.extend_from_slice(&cd_off.to_le_bytes());
    z.extend_from_slice(&[0, 0]);
    z
}

#[test]
fn parses_a_zip_wrapped_fig() {
    let bare = build_fig(&build_schema(), &build_data());
    let zipped = wrap_in_zip(&bare);
    // The ZIP form must be recognised + routed through the parser.
    let import = parse_fig_binary(&zipped, "Zipped", FigLayoutMode::OpenPencil)
        .expect("zip-wrapped .fig parses");
    let pages = import.document.pages.expect("has pages");
    assert_eq!(pages[0].name, "Page 1");
    assert_eq!(pages[0].children.len(), 1);
}

#[test]
fn document_serializes_to_canonical_json() {
    let fig = build_fig(&build_schema(), &build_data());
    let import = parse_fig_binary(&fig, "Test", FigLayoutMode::OpenPencil).unwrap();
    // The converted document must round-trip through the canonical
    // schema's serde representation.
    let json = serde_json::to_string(&import.document).expect("serializes");
    assert!(json.contains("\"Page 1\""));
    assert!(json.contains("\"Box\""));
}

#[test]
fn owned_source_preserves_bare_zip_pages_phases_and_canonical_output() {
    use crate::{prepare_fig_binary_owned, prepare_fig_binary_owned_with_observer};
    let bare = build_fig(&build_schema(), &build_data());
    for source in [bare.clone(), wrap_in_zip(&bare)] {
        let (mut borrowed_phases, mut owned_phases) = (Vec::new(), Vec::new());
        let borrowed =
            prepare_fig_binary_with_observer(&source, "Owned", FigLayoutMode::Preserve, &mut |p| {
                borrowed_phases.push(p)
            })
            .unwrap();
        let owned = prepare_fig_binary_owned_with_observer(
            source,
            "Owned",
            FigLayoutMode::Preserve,
            &mut |p| owned_phases.push(p),
        )
        .unwrap();
        assert_eq!(borrowed.pages(), owned.pages());
        assert_eq!(
            owned_phases,
            [
                FigImportPhase::Container,
                FigImportPhase::Schema,
                FigImportPhase::Decode,
                FigImportPhase::Extract,
                FigImportPhase::Prepared
            ]
        );
        assert!(borrowed_phases.contains(&FigImportPhase::BuildTree));
        let (borrowed, owned) = (borrowed.into_page(0).unwrap(), owned.into_page(0).unwrap());
        assert_eq!(
            serde_json::to_vec(&borrowed.document).unwrap(),
            serde_json::to_vec(&owned.document).unwrap()
        );
        assert_eq!(borrowed.warnings, owned.warnings);
    }
    assert!(matches!(
        prepare_fig_binary_owned(b"unknown".to_vec(), "Bad", FigLayoutMode::Preserve),
        Err(FigParseError::UnknownFormat)
    ));
}

#[test]
fn indexed_owned_all_pages_retains_eager_conversion_and_strict_errors() {
    let bare = build_fig(&build_schema(), &build_data());
    for mode in [FigLayoutMode::Preserve, FigLayoutMode::OpenPencil] {
        let eager =
            crate::figma_types::parse_fig_file_owned_with_observer(bare.clone(), &mut |_| {})
                .unwrap();
        let expected = crate::node_mapper::figma_all_pages_to_pen_document(eager, "All", mode);
        let actual = crate::prepare_fig_binary_owned(bare.clone(), "All", mode)
            .unwrap()
            .into_all_pages()
            .unwrap();
        assert_eq!(
            serde_json::to_value(actual.document).unwrap(),
            serde_json::to_value(expected.document).unwrap()
        );
        assert_eq!(actual.warnings, expected.warnings);
        assert!(matches!(
            crate::prepare_fig_binary_owned(bare.clone(), "All", mode)
                .unwrap()
                .into_page(99),
            Err(FigParseError::PageOutOfBounds {
                index: 99,
                page_count: 1
            })
        ));
    }
}

#[test]
fn indexed_owned_falls_back_for_duplicate_identities_and_alternate_root_arrays() {
    let mut duplicate = build_data();
    duplicate[1] = 4;
    duplicate.pop();
    let mut writer = W { out: duplicate };
    encode_node(
        &mut writer,
        &NodeSpec {
            local_id: 3,
            parent: Some((0, 2, "a")),
            type_value: 4,
            name: "Latest box",
            size: Some((75.0, 25.0)),
            transform: None,
        },
    );
    writer.var_uint(0);
    let mut renamed_schema = build_schema();
    let start = renamed_schema
        .windows(11)
        .position(|w| w == b"nodeChanges")
        .unwrap();
    renamed_schema.splice(start..start + 11, b"futureNodes".iter().copied());
    for source in [
        build_fig(&build_schema(), &writer.out),
        build_fig(&renamed_schema, &build_data()),
    ] {
        let expected = prepare_fig_binary(&source, "Fallback", FigLayoutMode::Preserve).unwrap();
        let actual =
            crate::prepare_fig_binary_owned(source, "Fallback", FigLayoutMode::Preserve).unwrap();
        assert_eq!(actual.pages(), expected.pages());
        let (expected, actual) = (
            expected.into_all_pages().unwrap(),
            actual.into_all_pages().unwrap(),
        );
        assert_eq!(
            serde_json::to_value(actual.document).unwrap(),
            serde_json::to_value(expected.document).unwrap()
        );
        assert_eq!(actual.warnings, expected.warnings);
    }
}
