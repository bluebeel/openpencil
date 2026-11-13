//! Export an imported Figma node through the production native renderer.
//! See --node-path for strict descendant selection; legacy top-level names work.

#[path = "render_fig_frame/cli.rs"]
mod cli;
#[path = "render_fig_frame/error.rs"]
mod error;
#[path = "render_fig_frame/selector.rs"]
mod selector;
#[path = "render_fig_frame/selector_error.rs"]
mod selector_error;

use cli::Arguments;
use error::{IoOperation, RenderFigError};
use jian_ops_schema::node::PenNode;
use op_editor_core::PenNodeExt;
use op_figma::{prepare_fig_binary_with_observer, FigImportPhase, FigLayoutMode};
use op_host_services::export::{
    export_node_raster_with_margin, export_node_raster_with_viewport, node_bounds, RasterFormat,
};
use std::path::Path;
use std::time::Instant;

fn count_nodes(node: &PenNode) -> usize {
    1 + node
        .children()
        .map(|children| children.iter().map(count_nodes).sum())
        .unwrap_or(0)
}
fn stage(name: &str, milliseconds: Option<f64>) {
    eprintln!(
        "render-stage:{}",
        serde_json::json!({"stage":name,"milliseconds":milliseconds})
    );
}
fn phase(args: &Arguments, name: &str) {
    if args.phase_events {
        println!(
            "{}",
            serde_json::json!({"event":"phase-start","phase":name})
        );
    }
}
fn run() -> Result<serde_json::Value, RenderFigError> {
    let args = Arguments::parse(std::env::args().skip(1).collect())?;
    phase(&args, "fonts");
    let started = Instant::now();
    let mut fonts = Vec::new();
    for path in &args.fonts {
        let bytes = std::fs::read(path)
            .map_err(|source| RenderFigError::io(IoOperation::ReadFont, path.clone(), source))?;
        let face = jian_skia::register_imported_font(bytes).map_err(|reason| {
            RenderFigError::FontRegistration {
                path: path.clone(),
                reason,
            }
        })?;
        fonts.push(serde_json::json!({"path":path,"family":face.family,"weight":face.weight}));
    }
    let font_ms = started.elapsed().as_secs_f64() * 1000.0;
    stage("fonts-loaded", Some(font_ms));
    phase(&args, "read");
    let started = Instant::now();
    let bytes = std::fs::read(&args.input)
        .map_err(|source| RenderFigError::io(IoOperation::ReadInput, args.input.clone(), source))?;
    let read_ms = started.elapsed().as_secs_f64() * 1000.0;
    phase(&args, "prepare");
    let started = Instant::now();
    stage("prepare-started", None);
    let import_started = Instant::now();
    let mut import_phases = Vec::new();
    let mut import_observer = |import_phase: FigImportPhase| {
        let elapsed_ms = import_started.elapsed().as_secs_f64() * 1000.0;
        if args.phase_events {
            println!(
                "{}",
                serde_json::json!({"event":"phase-start","phase":import_phase.as_str(),"elapsedMs":elapsed_ms})
            );
        }
        import_phases
            .push(serde_json::json!({"phase":import_phase.as_str(),"elapsedMs":elapsed_ms}));
    };
    let prepared = prepare_fig_binary_with_observer(
        &bytes,
        "Figma node export",
        FigLayoutMode::Preserve,
        &mut import_observer,
    )
    .map_err(RenderFigError::Prepare)?;
    drop(bytes);
    let prepare_ms = started.elapsed().as_secs_f64() * 1000.0;
    stage("prepared", Some(prepare_ms));
    let pages: Vec<_> = prepared
        .pages()
        .iter()
        .enumerate()
        .filter(|(_, page)| page.id == args.page || page.name == args.page)
        .collect();
    if pages.len() != 1 {
        return Err(RenderFigError::PageMatchCount {
            selector: args.page.clone(),
            matches: pages.len(),
        });
    }
    let page_index = pages[0].0;
    let source_page_id = pages[0].1.id.clone();
    let source_page_name = pages[0].1.name.clone();
    phase(&args, "convert");
    let started = Instant::now();
    stage("convert-started", None);
    let import = prepared
        .into_page_with_observer(page_index, &mut import_observer)
        .map_err(RenderFigError::Convert)?;
    let convert_ms = started.elapsed().as_secs_f64() * 1000.0;
    stage("converted", Some(convert_ms));
    if let Ok(path) = std::env::var("OP_DUMP_JSON") {
        let bytes =
            serde_json::to_vec_pretty(&import.document).map_err(RenderFigError::DumpSerialize)?;
        std::fs::write(&path, bytes)
            .map_err(|source| RenderFigError::io(IoOperation::WriteDocument, path, source))?;
    }
    let page = import
        .document
        .pages
        .as_ref()
        .and_then(|pages| pages.first())
        .ok_or(RenderFigError::ConvertedPageMissing)?;
    let selected = selector::select(&page.children, &args.selector)?;
    let frame_id = selected.node.base().id.clone();
    let frame_name = selected.node.base().name.clone();
    let selected_path = selected.path;
    let frame_node_count = count_nodes(selected.node);
    phase(&args, "layout");
    let started = Instant::now();
    let mut state = op_editor_core::EditorState::from_document(import.document);
    state.editor_ui.preserve_authored_geometry = true;
    let scene = op_pen_loader::editor_state_to_active_page_layout_scene(&state);
    let layout_ms = started.elapsed().as_secs_f64() * 1000.0;
    stage("layout-resolved", Some(layout_ms));
    if let Ok(path) = std::env::var("OP_DUMP_SCENE") {
        std::fs::write(&path, format!("{scene:#?}"))
            .map_err(|source| RenderFigError::io(IoOperation::WriteScene, path, source))?;
    }
    let node = scene
        .active_page()
        .and_then(|page| page.find(&frame_id))
        .ok_or_else(|| RenderFigError::SceneNodeMissing {
            id: frame_id.clone(),
        })?;
    if node.hidden {
        return Err(RenderFigError::HiddenNode { id: frame_id });
    }
    let bounds = if args.authored_bounds {
        node.bounds
    } else {
        node_bounds(node).ok_or_else(|| RenderFigError::NoPaintedBounds {
            id: frame_id.clone(),
        })?
    };
    let actual_bounds = [
        bounds.origin.x as f64,
        bounds.origin.y as f64,
        bounds.size.x as f64,
        bounds.size.y as f64,
    ];
    args.check_bounds(actual_bounds)?;
    phase(&args, "render");
    let started = Instant::now();
    stage("render-started", None);
    if args.authored_bounds {
        export_node_raster_with_viewport(
            &scene,
            &frame_id,
            Path::new(&args.output),
            RasterFormat::Png,
            args.scale,
            bounds,
        )?;
    } else {
        export_node_raster_with_margin(
            &scene,
            &frame_id,
            Path::new(&args.output),
            RasterFormat::Png,
            args.scale,
            0.0,
        )?;
    }
    let render_ms = started.elapsed().as_secs_f64() * 1000.0;
    let pixels = cli::png_dimensions(Path::new(&args.output))?;
    args.check_pixels(pixels)?;
    stage("rendered", Some(render_ms));
    Ok(serde_json::json!({
        "status":"ok","pageId":source_page_id,"pageName":source_page_name,
        "frameName":frame_name,"frameId":frame_id,"frameNodeCount":frame_node_count,
        "selectedPath":selected_path,"output":args.output,"bounds":actual_bounds,
        "pixels":{"width":pixels.0,"height":pixels.1},"scale":args.scale,
        "preserveAuthoredGeometry":true,"warnings":import.warnings,"fonts":fonts,
        "exportBounds":if args.authored_bounds { "authored" } else { "painted" },
        "resourceInstrumentation":"Registered font files and importer warnings; per-style fallback events are not exposed",
        "importPhases":import_phases,
        "timingsMs":{"read":read_ms,"fonts":font_ms,"prepare":prepare_ms,"load":read_ms+prepare_ms,"convert":convert_ms,"layout":layout_ms,"render":render_ms}
    }))
}
fn main() {
    match run() {
        Ok(result) => println!("{result}"),
        Err(error) => {
            eprintln!("render error: {error}");
            println!(
                "{}",
                serde_json::json!({"status":"error","error":error.to_string(),"errorDetail":error.detail()})
            );
            std::process::exit(1);
        }
    }
}
