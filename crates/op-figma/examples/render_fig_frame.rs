//! Export one imported Figma frame through the production native renderer.
//!
//! Usage: `cargo run -p op-figma --example render_fig_frame --
//! <input.fig> <page-id-or-name> <top-level-frame-name> <output.png> [font.ttf ...]`

use jian_ops_schema::node::PenNode;
use op_editor_core::PenNodeExt;
use op_figma::{prepare_fig_binary, FigLayoutMode};
use op_host_services::export::{export_node_raster_with_margin, node_bounds, RasterFormat};
use std::path::Path;
use std::time::Instant;

fn count_nodes(node: &PenNode) -> usize {
    1 + node
        .children()
        .map(|children| children.iter().map(count_nodes).sum())
        .unwrap_or(0)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(
        args.len() >= 4,
        "usage: render_fig_frame <input.fig> <page-id-or-name> <frame-name> <output.png> [font.ttf ...]"
    );
    for path in &args[4..] {
        let bytes = std::fs::read(path).expect("read font");
        let face = jian_skia::register_imported_font(bytes).expect("register font");
        eprintln!("font: {} {}", face.family, face.weight);
    }

    let bytes = std::fs::read(&args[0]).expect("read .fig");
    let started = Instant::now();
    let prepared = prepare_fig_binary(&bytes, "Figma frame export", FigLayoutMode::Preserve)
        .expect("prepare .fig");
    let prepared_ms = started.elapsed().as_secs_f64() * 1000.0;
    eprintln!("prepared .fig: {prepared_ms:.1}ms");
    let page_index = prepared
        .pages()
        .iter()
        .position(|page| page.id == args[1] || page.name == args[1])
        .expect("find page by id or name");
    let source_page_id = prepared.pages()[page_index].id.clone();
    let started = Instant::now();
    let import = prepared.into_page(page_index).expect("convert page");
    let converted_ms = started.elapsed().as_secs_f64() * 1000.0;
    eprintln!("converted page: {converted_ms:.1}ms");
    if let Ok(path) = std::env::var("OP_DUMP_JSON") {
        let bytes = serde_json::to_vec_pretty(&import.document).expect("serialize document");
        std::fs::write(path, bytes).expect("write imported document");
    }
    let page = &import.document.pages.as_ref().expect("imported pages")[0];
    let targets: Vec<_> = page
        .children
        .iter()
        .filter(|node| node.base().name.as_deref() == Some(args[2].as_str()))
        .collect();
    assert_eq!(
        targets.len(),
        1,
        "frame name must select one top-level node"
    );
    let frame_id = targets[0].base().id.clone();
    let frame_node_count = count_nodes(targets[0]);
    let started = Instant::now();
    let mut state = op_editor_core::EditorState::from_document(import.document);
    state.editor_ui.preserve_authored_geometry = true;
    let scene = op_pen_loader::editor_state_to_active_page_layout_scene(&state);
    if let Ok(path) = std::env::var("OP_DUMP_SCENE") {
        std::fs::write(path, format!("{scene:#?}")).expect("write scene diagnostics");
    }
    let layout_ms = started.elapsed().as_secs_f64() * 1000.0;
    eprintln!("resolved layout: {layout_ms:.1}ms");
    let node = scene.active_page().unwrap().find(&frame_id).unwrap();
    let bounds = node_bounds(node).expect("frame has painted bounds");
    let started = Instant::now();
    export_node_raster_with_margin(
        &scene,
        &frame_id,
        Path::new(&args[3]),
        RasterFormat::Png,
        1.0,
        0.0,
    )
    .expect("export PNG");
    let render_ms = started.elapsed().as_secs_f64() * 1000.0;
    println!(
        "{}",
        serde_json::json!({
            "pageId": source_page_id,
            "frameName": args[2],
            "frameId": frame_id,
            "frameNodeCount": frame_node_count,
            "output": args[3],
            "bounds": [bounds.origin.x, bounds.origin.y, bounds.size.x, bounds.size.y],
            "warnings": import.warnings,
            "timingsMs": { "prepare": prepared_ms, "convert": converted_ms, "layout": layout_ms, "render": render_ms },
        })
    );
}
