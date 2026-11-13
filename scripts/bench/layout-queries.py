#!/usr/bin/env python3
"""Computed-layout MCP queries on synthetic flat/nested 10k/100k documents.

Uses the real host font measure, component resolver and scene geometry. Outputs
full response hashes for --baseline comparison; this is not authored-bounds math.
Run under a process-tree resource guard. --case bounds a known slow baseline.
"""
import argparse
import asyncio
import importlib.util
import json
from pathlib import Path

spec = importlib.util.spec_from_file_location("node_query_benchmark", Path(__file__).with_name("node-queries.py"))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)
shared = runner.shared
TOPOLOGY = "flat"
CASE_FILTER = None


def fixture(count):
    # Reserve10control nodes and one enclosing board only in nested topology.
    ordinary = count - (11 if TOPOLOGY == "nested" else 10)
    middle = (ordinary + 1) // 2
    rects = [{"id": f"r{i}", "type": "rectangle", "x": (i % 100) * 8,
              "y": (i // 100) * 8, "width": 6, "height": 6} for i in range(ordinary)]
    measure = {"id": "measure", "type": "frame", "name": "Measured flex", "x": 321.25, "y": 123.5,
        "width": 400, "height": "fit_content", "layout": "vertical", "padding": [10, 20], "gap": 8,
        "children": [{"id": "measured-title", "type": "text", "name": "Measured text",
            "content": "Measured AV ffi", "fontFamily": "Inter", "fontSize": 18,
            "width": "fit_content", "height": "fit_content"},
            {"id": "measure-row", "type": "frame", "layout": "horizontal", "width": "fill_container",
             "height": 60, "gap": 6, "padding": 4, "children": [
                {"id": "flex-a", "type": "rectangle", "width": "fill_container", "height": 12},
                {"id": "instance", "type": "ref", "ref": "master", "width": 80, "height": 40}]}]}
    first = rects[:middle] + [measure]
    if TOPOLOGY == "nested":
        first = [{"id": "board", "type": "frame", "x": 101.5, "y": 51.25,
                  "width": 9000, "height": 9000, "children": first}]
    return {"version": "1.0.0", "children": [], "pages": [
        {"id": "first", "name": "First", "children": first},
        {"id": "other", "name": "Other", "children": rects[middle:] + [
            {"id": "other-measure", "type": "frame", "x": 900, "y": 100, "width": 60, "height": 50,
             "children": [{"id": "other-leaf", "type": "rectangle", "x": 12, "y": 14, "width": 20, "height": 30}]}]},
        {"id": "library", "name": "Library", "children": [
            {"id": "master", "type": "frame", "reusable": True, "width": 80, "height": 40, "children": [
                {"id": "master-inner", "type": "frame", "width": 40, "height": 30, "children": [
                    {"id": "master-leaf", "type": "rectangle", "x": 3, "y": 2, "width": 6, "height": 7}]}]}]}]}


def cases(count):
    values = [
        ("layout-small-parent-depth-zero", "snapshot_layout", {"parentId": "measure", "maxDepth": 0}, ["measured-title", "measure-row"]),
        ("layout-small-parent-depth-one", "snapshot_layout", {"parentId": "measure", "maxDepth": 1}, ["measured-title", "measure-row"]),
        ("layout-virtual-parent", "snapshot_layout", {"parentId": "instance__master-inner", "maxDepth": 0}, None),
        ("layout-inactive-page", "snapshot_layout", {"pageId": "other", "parentId": "other-measure", "maxDepth": 0}, ["other-leaf"]),
        ("layout-wide-depth-zero", "snapshot_layout", {"maxDepth": 0}, None),
        ("layout-wide-depth-one", "snapshot_layout", {"maxDepth": 1}, None),
        ("layout-page-error", "snapshot_layout", {"pageId": "absent", "maxDepth": "invalid"}, "page not found: absent"),
        ("layout-depth-error", "snapshot_layout", {"maxDepth": "invalid"}, "maxDepth must be a u32"),
        ("layout-parent-error", "snapshot_layout", {"parentId": "absent", "maxDepth": 0}, "Node not found: absent"),
        ("empty-small-node", "find_empty_space", {"nodeId": "measure", "width": 50, "height": 30, "padding": 6, "direction": "bottom"}, None),
        ("empty-inactive-node", "find_empty_space", {"pageId": "other", "nodeId": "other-measure", "width": 50, "height": 30, "padding": 6, "direction": "right"}, {"x": "966", "y": "100"}),
        ("empty-width-error", "find_empty_space", {"pageId": "absent", "height": 30, "direction": "right"}, "width is required"),
        ("canvas-bounds", "get_canvas_bounds", {}, None),
    ]
    return [case for case in values if CASE_FILTER is None or case[0] in CASE_FILTER]


def validate(label, response, expected):
    if isinstance(expected, str):
        assert response.get("isError") and expected in response["message"], response
        return
    assert not response.get("isError"), response
    if isinstance(expected, dict):
        for key, value in expected.items(): assert response[key] == value, (label, response)
    if label.startswith("layout-"):
        layout = response["layout"]
        assert isinstance(layout, list)
        if isinstance(expected, list): assert [node["id"] for node in layout] == expected, response
        if label.startswith("layout-small-parent"):
            assert layout[0]["width"] > 30 and layout[0]["width"] != 100, "real font measure required"
            assert layout[1]["width"] >= 350, "fill_container must use computed flex width"
            assert layout[0]["x"] == 20 and layout[0]["y"] == 10, "parent-relative coordinates"
            if label.endswith("zero"): assert "children" not in layout[1]
            else: assert [node["id"] for node in layout[1]["children"]] == ["flex-a", "instance"]
        elif label == "layout-inactive-page":
            assert layout[0]["x"] == 12 and layout[0]["y"] == 14
        elif label == "layout-virtual-parent":
            assert len(layout) == 1 and layout[0]["id"].endswith("master-leaf")
            assert "name" not in layout[0], "legacy virtual-node metadata omits the name"
        elif label.startswith("layout-wide"):
            assert layout and layout[-1]["id"] == ("board" if TOPOLOGY == "nested" else "measure")
    elif label == "canvas-bounds":
        assert response["has_content"] == "true" and int(response["w"]) > 300
    elif label == "empty-small-node":
        # The nested board adds an origin; both cases must use measured height,
        # rather than the old raw missing-size default of100px.
        assert int(response["y"]) > (229 if TOPOLOGY == "flat" else 280), response


async def main_async(args):
    global TOPOLOGY, CASE_FILTER
    args.output.mkdir(parents=True, exist_ok=False)
    CASE_FILTER = set(args.case) if args.case else None
    runner.fixture, runner.cases, runner.validate = fixture, cases, validate
    reports = []
    for topology in args.topology:
        TOPOLOGY = topology
        for count in args.nodes:
            child = argparse.Namespace(**vars(args))
            child.output = args.output / topology
            baseline_file = args.baseline / topology / str(count) / "report.json" if args.baseline else None
            baseline = json.loads(baseline_file.read_text()) if baseline_file and baseline_file.exists() else None
            report = await asyncio.wait_for(runner.run_case(child, count, baseline), args.timeout)
            report["topology"] = topology
            reports.append(report)
            (args.output / "summary.json").write_text(json.dumps({"results": reports}, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--nodes", type=int, nargs="+", default=[10000, 100000])
    parser.add_argument("--topology", choices=["flat", "nested"], nargs="+", default=["flat", "nested"])
    parser.add_argument("--case", nargs="+", help="case labels, e.g. layout-small-parent-depth-zero")
    parser.add_argument("--iterations", type=int, default=3)
    parser.add_argument("--max-bytes", type=int, default=1_000_000_000)
    parser.add_argument("--timeout", type=float, default=90)
    args = parser.parse_args()
    assert all(32 <= count <= 100000 for count in args.nodes)
    asyncio.run(main_async(args))


if __name__ == "__main__": main()
