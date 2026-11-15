//! Canonical scene hit/spatial-query benchmark; no UI, importer or raster work.
//!
//! Build: cargo build -j1 -p op-pen-loader --example scene_queries
//! Run through scripts/bench/node-queries.py --scene-binary <this binary> to
//! collect process CPU/RSS/physical-footprint alongside the per-query timings.

use std::time::Instant;

use jian_scene::layout_scene::{LayoutScene, NodeKind, SceneNode, ScenePage};
use op_editor_core::render_backend::{Color, Point2D, Rect};
use serde_json::json;

fn scene(count: usize) -> LayoutScene {
    let children = (0..count)
        .map(|index| {
            let mut node = SceneNode::leaf(format!("n{}", index + 1), NodeKind::Rect);
            node.bounds = Rect::xywh(
                (index % 1000) as f32 * 8.0,
                (index / 1000) as f32 * 8.0,
                6.0,
                6.0,
            );
            node.fill = Some(Color::BLACK);
            node
        })
        .collect();
    LayoutScene {
        pages: vec![ScenePage {
            id: "grid".into(),
            name: "Grid".into(),
            children,
        }],
        active_page_index: 0,
    }
}

fn validate_geometry_contracts() {
    let mut back = SceneNode::leaf("back", NodeKind::Rect);
    back.bounds = Rect::xywh(0.0, 0.0, 20.0, 20.0);
    back.fill = Some(Color::BLACK);
    let mut front = back.clone();
    front.id = "front".into();
    let mut scene = LayoutScene {
        pages: vec![ScenePage {
            id: "p".into(),
            name: "P".into(),
            children: vec![front, back],
        }],
        active_page_index: 0,
    };
    assert_eq!(
        scene
            .node_at_doc_point(Point2D::new(10.0, 10.0), 1.0)
            .as_deref(),
        Some("front")
    );
    scene.pages[0].children[0].hidden = true;
    assert_eq!(
        scene
            .node_at_doc_point(Point2D::new(10.0, 10.0), 1.0)
            .as_deref(),
        Some("back")
    );
    scene.pages[0].children[0].hidden = false;
    scene.pages[0].children[0].locked = true;
    assert_eq!(
        scene
            .node_at_doc_point(Point2D::new(10.0, 10.0), 1.0)
            .as_deref(),
        Some("back")
    );
}

fn measure(count: usize, iterations: usize) -> serde_json::Value {
    validate_geometry_contracts();
    let scene = scene(count);
    let start = Instant::now();
    for iteration in 0..iterations {
        let index = (iteration * 7919) % count;
        let point = Point2D::new(
            (index % 1000) as f32 * 8.0 + 3.0,
            (index / 1000) as f32 * 8.0 + 3.0,
        );
        assert_eq!(
            scene.node_at_doc_point(point, 1.0),
            Some(format!("n{}", index + 1))
        );
    }
    let hit = start.elapsed().as_secs_f64();
    let start = Instant::now();
    for iteration in 0..iterations {
        let index = (iteration * 7919) % count;
        let point = Point2D::new(
            (index % 1000) as f32 * 8.0 + 3.0,
            (index / 1000) as f32 * 8.0 + 3.0,
        );
        assert_eq!(
            scene.node_path_at_doc_point(point, 1.0),
            Some(vec![format!("n{}", index + 1)])
        );
    }
    let path = start.elapsed().as_secs_f64();
    let start = Instant::now();
    for _ in 0..iterations {
        assert_eq!(
            scene.node_at_doc_point(Point2D::new(-10.0, -10.0), 1.0),
            None
        );
    }
    let miss = start.elapsed().as_secs_f64();
    let start = Instant::now();
    let columns = 1000usize.min(count);
    let rows = count.div_ceil(1000);
    for iteration in 0..iterations {
        let column = (iteration * 17) % (columns - 1).max(1);
        let row = (iteration * 13) % (rows - 1).max(1);
        let rect = Rect::xywh(
            column as f32 * 8.0 + 2.0,
            row as f32 * 8.0 + 2.0,
            12.0,
            12.0,
        );
        let actual = scene.nodes_intersecting_doc_rect(rect);
        let expected: Vec<_> = [
            row * 1000 + column,
            row * 1000 + column + 1,
            (row + 1) * 1000 + column,
            (row + 1) * 1000 + column + 1,
        ]
        .into_iter()
        .filter(|index| *index < count)
        .map(|index| format!("n{}", index + 1))
        .collect();
        assert_eq!(actual, expected);
    }
    let spatial = start.elapsed().as_secs_f64();
    json!({
        "correctness": true, "sceneNodes": count, "queriesPerKind": iterations,
        "sceneNodeSizeBytes": std::mem::size_of::<SceneNode>(),
        "scope": "Canonical LayoutScene input geometry; no layout/import/raster/network work",
        "timings": {
            "hit": {"wallSeconds": hit, "secondsPerQuery": hit / iterations as f64},
            "hitPath": {"wallSeconds": path, "secondsPerQuery": path / iterations as f64},
            "miss": {"wallSeconds": miss, "secondsPerQuery": miss / iterations as f64},
            "rectIntersection": {"wallSeconds": spatial, "secondsPerQuery": spatial / iterations as f64}
        }
    })
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let count: usize = arguments
        .next()
        .as_deref()
        .unwrap_or("10000")
        .parse()
        .expect("node count");
    let iterations: usize = arguments
        .next()
        .as_deref()
        .unwrap_or("256")
        .parse()
        .expect("query count");
    assert!((16..=100_000).contains(&count) && (1..=10_000).contains(&iterations));
    println!("{}", measure(count, iterations));
}
