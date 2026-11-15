#!/usr/bin/env python3
"""Measure production MCP node queries without renderer/export work.

One real shared serve-web authority per synthetic 10k/100k document. Responses
are checked semantically and hashed in full; --baseline compares exact normalized
response hashes after an implementation change. No corpus or external assets.
"""
import argparse
import asyncio
import gc
import hashlib
import importlib.util
import json
from pathlib import Path
import resource
import sys
import time

spec = importlib.util.spec_from_file_location("shared_benchmark", Path(__file__).with_name("headless-agents.py"))
shared = importlib.util.module_from_spec(spec)
spec.loader.exec_module(shared)


def fixture(count):
    ordinary = count - 4
    middle = (ordinary + 1) // 2
    roots = [{"type": "rectangle", "id": f"n{i}", "name": f"Rectangle {i}",
              "x": (i % 1000) * 8, "y": (i // 1000) * 8, "width": 6, "height": 6}
             for i in range(1, ordinary + 1)]
    detail = {"type": "frame", "id": "detail-root", "name": "Detail", "width": 100, "height": 100,
              "children": [{"type": "frame", "id": "detail-group", "name": "Group", "width": 80,
                            "height": 80, "children": [{"type": "rectangle", "id": "detail-leaf",
                            "name": "Leaf token", "width": 12, "height": 14,
                            "fill": [{"type": "solid", "color": "$brand"}]}]}]}
    return {"version": "1.0.0", "variables": {"brand": {"type": "color", "value": "#3366ff"}},
            "children": [], "pages": [{"id": "first", "name": "First", "children": [{"type": "frame", "id": "bulk-root", "name": "Bulk", "width": 8000, "height": 8000, "children": roots[:middle]}]},
            {"id": "second", "name": "Second", "children": roots[middle:]},
            {"id": "detail", "name": "Detail", "children": [detail]},
            {"id": "empty", "name": "Empty", "children": []}]}


def cases(count):
    last = f"n{count - 4}"
    last_active = (count - 3) // 2
    return [
        ("get-last-page-node", "get_node", {"node_id": last}, {"name": f"Rectangle {count - 4}", "parent_id": ""}),
        ("get-nested-leaf", "get_node", {"node_id": "detail-leaf"}, {"name": "Leaf token", "parent_id": "detail-group"}),
        ("read-large-parent-depth-zero", "read_nodes", {"nodeIds": ["bulk-root"], "depth": 0}, ["bulk-root"]),
        ("read-three", "read_nodes", {"nodeIds": ["n3", "n1", "n3", "absent"], "depth": 0}, ["n3", "n1", "n3"]),
        ("read-other-page", "read_nodes", {"pageId": "second", "nodeIds": [last], "depth": 0}, [last]),
        ("read-wrong-page", "read_nodes", {"nodeIds": [last]}, []),
        ("read-depth-zero", "read_nodes", {"pageId": "detail", "nodeIds": ["detail-root"], "depth": 0}, ["detail-root"]),
        ("read-depth-one", "read_nodes", {"pageId": "detail", "nodeIds": ["detail-root"], "depth": 1}, ["detail-root"]),
        ("read-alias-metadata", "read_nodes", {"page_id": "detail", "node_ids": "detail-leaf,detail-root", "include_variables": True}, ["detail-leaf", "detail-root"]),
        ("read-empty-ids", "read_nodes", {"pageId": "empty", "nodeIds": []}, []),
        ("read-top-level", "read_nodes", {"pageId": "detail", "depth": 0}, ["detail-root"]),
        ("batch-three", "batch_get", {"nodeIds": ["n3", "n1", "n3", "absent"], "readDepth": 0}, ["n3", "n1"]),
        ("batch-other-page", "batch_get", {"pageId": "second", "nodeIds": [last], "readDepth": 0}, [last]),
        ("batch-parent-search", "batch_get", {"pageId": "detail", "parentId": "detail-root", "patterns": [{"name": "^leaf"}], "searchDepth": 1, "readDepth": 0}, ["detail-leaf"]),
        ("batch-mixed-order", "batch_get", {"pageId": "detail", "patterns": [{"name": "^leaf"}], "nodeIds": ["detail-root", "detail-leaf"], "readDepth": 0}, ["detail-leaf", "detail-root"]),
        ("batch-token-resolve", "batch_get", {"pageId": "detail", "nodeIds": ["detail-leaf"], "resolveRefs": True}, ["detail-leaf"]),
        ("selection-bulk-depth-zero", "get_selection", {"readDepth": 0}, ["bulk-root"]),
        ("selection-bulk-invalid-depth", "get_selection", {"readDepth": "bad"}, "readDepth must be an i32"),
        ("get-parent-last-active", "get_node_parent", {"node_id": f"n{last_active}"}, {"parent_id": "bulk-root", "depth": "1"}),
        ("get-parent-ignores-other-page", "get_node_parent", {"node_id": "detail-leaf", "pageId": "detail"}, "not found on active page"),
        ("get-children-small-container", "get_node_children", {"node_id": "detail-root"}, {"count": "1", "ids": "detail-group", "child_0_name": "Group"}),
        ("get-children-leaf", "get_node_children", {"node_id": last}, {"count": "0", "ids": ""}),
        ("get-children-missing", "get_node_children", {"node_id": "absent"}, "node absent not found"),
        ("find-name-first", "find_node_by_name", {"name": "Bulk"}, {"id": "bulk-root", "kind": "frame"}),
        ("find-name-last-active", "find_node_by_name", {"name": f"Rectangle {last_active}"}, {"id": f"n{last_active}", "kind": "rectangle"}),
        ("find-name-other-page", "find_node_by_name", {"name": "Leaf token"}, "on the active page"),
        ("get-node-camel-alias-error", "get_node", {"nodeId": "n1"}, "node_id is required"),
        ("get-node-missing-arg", "get_node", {}, "node_id is required"),
        ("missing-node-error", "get_node", {"node_id": "absent"}, "node absent not found"),
        ("page-error-priority", "read_nodes", {"pageId": "absent", "depth": "bad"}, "page not found: absent"),
        ("depth-error", "read_nodes", {"depth": "bad"}, "depth must be an i32"),
        ("ids-error", "read_nodes", {"nodeIds": '["n1",1]'}, "nodeIds must be a JSON string array"),
        ("pattern-error", "batch_get", {"patterns": '[{"name":"["}]'}, "pattern name must be a valid regex"),
    ]


def validate(label, response, expected):
    if isinstance(expected, str):
        assert response.get("isError") is True and expected in response["message"], response
        return
    assert not response.get("isError"), response
    if isinstance(expected, dict):
        for key, value in expected.items():
            assert response[key] == value, (label, key, response)
        return
    assert [node["id"] for node in response["nodes"]] == expected, (label, response)
    assert "count" not in response
    if label in ("read-depth-zero", "read-top-level", "read-large-parent-depth-zero", "selection-bulk-depth-zero"):
        assert response["nodes"][0]["children"] == "..."
    elif label == "read-depth-one":
        assert response["nodes"][0]["children"][0]["id"] == "detail-group"
        assert response["nodes"][0]["children"][0]["children"] == "..."
    elif label == "read-alias-metadata":
        assert response["variables"] == {"brand": {"type": "color", "value": "#3366ff"}}
        assert response["themes"] == []
        assert response["nodes"][0]["fill"][0]["color"] == "$brand"
    elif label == "batch-token-resolve":
        assert response["nodes"][0]["fill"][0]["color"] == "#3366ff"
    if label.startswith("selection-"):
        assert response["selectedIds"] == ["bulk-root"] and response["activePageId"] == "first"
    if label.startswith("read-") and label != "read-alias-metadata":
        assert "variables" not in response and "themes" not in response


async def run_case(args, count, baseline):
    output = args.output.resolve() / str(count)
    output.mkdir(parents=True, exist_ok=False)
    document = output / "queries.op"
    initial = fixture(count)
    document.write_text(json.dumps(initial, separators=(",", ":")))
    assert len(list(shared.nodes(initial))) == count
    initial_sha256 = shared.sha(document)
    del initial
    gc.collect()
    command = [str(args.binary.resolve()), "--serve-web", "--managed", "--port", "0", "--file", str(document),
               "--config-root", str(output / "config")]
    report = {"binary": str(args.binary.resolve()), "binarySha256": shared.sha(args.binary), "command": command,
              "initialNodes": count, "iterations": args.iterations, "oneSharedDocumentAuthority": True,
              "logicalClients": 1, "maxInflightRequests": 1, "externalAssets": False, "queries": {}}
    process = sampler = None
    log = (output / "server.stderr.log").open("wb")
    started = time.perf_counter()
    before_cpu = resource.getrusage(resource.RUSAGE_CHILDREN)
    try:
        shared.phase("startup")
        process = await asyncio.create_subprocess_exec(*command, stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE, stderr=log)
        sampler = asyncio.create_task(shared.monitor(process, report, args.max_bytes))
        handshake = json.loads(await asyncio.wait_for(process.stdout.readline(), 30))
        assert handshake.get("ok"), handshake
        port = handshake["port"]
        selection_ready = False
        for label, tool, arguments, expected in cases(count):
            if label.startswith("selection-") and not selection_ready:
                shared.phase("selection-ui-setup")
                await shared.tool(port, "selection-warmup", "set_selection", {"node_id": "bulk-root"})
                selection_ready = True
                report["selectionSetup"] = "One UI-only set_selection; no design-content edits"
            shared.phase(label)
            walls, cpus, hashes = [], [], set()
            response = None
            for iteration in range(args.iterations):
                before = shared.usage(process.pid)
                at = time.perf_counter()
                wire = await shared.request(port, "POST", "/mcp", {"jsonrpc": "2.0", "id": f"{label}-{iteration}",
                    "method": "tools/call", "params": {"name": tool, "arguments": arguments}})
                walls.append(time.perf_counter() - at)
                after = shared.usage(process.pid)
                cpus.append(after["cpuSeconds"] - before["cpuSeconds"] if before and after else 0)
                assert not wire.get("error"), wire
                result = wire["result"]
                text = result["content"][0]["text"]
                response = {"isError": True, "message": text} if result.get("isError") else json.loads(text)
                validate(label, response, expected)
                hashes.add(hashlib.sha256(json.dumps(response, sort_keys=True, separators=(",", ":")).encode()).hexdigest())
            assert len(hashes) == 1, (label, "responses changed without writes")
            digest = hashes.pop()
            previous = baseline.get("queries", {}).get(label) if baseline else None
            if previous:
                assert digest == previous["responseSha256"], (label, "baseline response changed")
            (output / f"{label}.json").write_text(json.dumps(response, indent=2) + "\n")
            report["queries"][label] = {"tool": tool, "arguments": arguments, "samples": len(walls),
                "wallP50Seconds": shared.percentile(walls, .5), "wallP95Seconds": shared.percentile(walls, .95),
                "serviceCpuP50Seconds": shared.percentile(cpus, .5), "responseSha256": digest,
                "responseBytes": len(json.dumps(response).encode()), "exactBaselineMatch": True if previous else None}
        assert shared.sha(document) == initial_sha256, "queries must not edit the file"
        report["documentSha256"] = initial_sha256
        report["correctness"] = True
    except BaseException as error:
        report.update(correctness=False, error=f"{type(error).__name__}: {error}")
        raise
    finally:
        if process:
            if process.stdin: process.stdin.close()
            try: await asyncio.wait_for(process.wait(), 10)
            except asyncio.TimeoutError:
                process.terminate()
                await asyncio.wait_for(process.wait(), 5)
            report["serverExitCode"] = process.returncode
        if sampler: await sampler
        report["correctness"] = report.get("correctness", False) and report.get("serverExitCode") == 0 and not report.get("memoryBudgetExceeded", False)
        log.close()
        after_cpu = resource.getrusage(resource.RUSAGE_CHILDREN)
        report["serviceCpuSeconds"] = after_cpu.ru_utime + after_cpu.ru_stime - before_cpu.ru_utime - before_cpu.ru_stime
        report["totalWallSeconds"] = time.perf_counter() - started
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    if not report.get("correctness"):
        raise RuntimeError("query case failed (including sampled or kernel high-water memory limits); see report")
    return report


async def run_scene_case(args, count):
    report = {"binary": str(args.scene_binary.resolve()), "binarySha256": shared.sha(args.scene_binary),
              "sceneNodes": count, "queriesPerKind": args.scene_queries}
    shared.phase("scene-hit-and-spatial-query")
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    started = time.perf_counter()
    process = await asyncio.create_subprocess_exec(str(args.scene_binary.resolve()), str(count), str(args.scene_queries),
        stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
    sampler = asyncio.create_task(shared.monitor(process, report, args.max_bytes))
    try:
        stdout, stderr = await asyncio.wait_for(process.communicate(), args.timeout)
        assert process.returncode == 0, (process.returncode, stderr.decode())
        result = json.loads(stdout)
        assert result["correctness"] is True and result["sceneNodes"] == count
        report.update(result)
    finally:
        if process.returncode is None:
            process.terminate()
            await process.wait()
        await sampler
        after = resource.getrusage(resource.RUSAGE_CHILDREN)
        report["childCpuSeconds"] = after.ru_utime + after.ru_stime - before.ru_utime - before.ru_stime
        report["totalWallSeconds"] = time.perf_counter() - started
        report["memoryScope"] = "canonical scene query process + Python load generator"
        (args.output / str(count) / "scene-queries.json").write_text(json.dumps(report, indent=2) + "\n")
    return report


async def suite(args):
    args.output.resolve().mkdir(parents=True, exist_ok=False)
    reports = []
    for count in args.nodes:
        baseline_path = args.baseline / str(count) / "report.json" if args.baseline else None
        baseline = json.loads(baseline_path.read_text()) if baseline_path and baseline_path.exists() else None
        reports.append(await asyncio.wait_for(run_case(args, count, baseline), args.timeout))
        if args.scene_binary:
            reports[-1]["canonicalSceneQueries"] = await run_scene_case(args, count)
        (args.output / "summary.json").write_text(json.dumps({"results": reports}, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--scene-binary", type=Path, help="built op-pen-loader scene_queries example; adds exact hit/spatial-query checks")
    parser.add_argument("--scene-queries", type=int, default=256)
    parser.add_argument("--baseline", type=Path, help="prior suite output; require exact normalized hashes for cases/counts present there")
    parser.add_argument("--nodes", type=int, nargs="+", default=[10000, 100000])
    parser.add_argument("--iterations", type=int, default=7)
    parser.add_argument("--max-bytes", type=int, default=1_000_000_000)
    parser.add_argument("--timeout", type=float, default=90)
    args = parser.parse_args()
    assert all(16 <= count <= 100000 for count in args.nodes) and 1 <= args.iterations <= 100
    asyncio.run(suite(args))


if __name__ == "__main__":
    main()
