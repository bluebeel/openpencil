#!/usr/bin/env python3
"""Concurrent drawing into several resident files through one headless process.

Shares HTTP/resource helpers with headless-agents.py. Checks every file through
MCP and on disk, renders each file, and never retries ambiguous writes. External
LLMs are excluded; the load generator is included in the memory budget.
"""
import argparse
import asyncio
import base64
from collections import Counter
import importlib.util
import json
from pathlib import Path
import resource
import sys
import time


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


shared = module("shared_multifile_benchmark", "headless-agents.py")
pixels = module("multifile_pixels", "operation-matrix.py")
COLORS = ["#2563eb", "#16a34a", "#e11d48", "#9333ea", "#ea580c", "#0891b2", "#ca8a04", "#475569"]


async def benchmark(args, clients, output):
    output.mkdir(parents=True, exist_ok=False)
    paths = [output / f"document-{i}.op" for i in range(args.files)]
    for i, path in enumerate(paths):
        path.write_text(json.dumps({"version": "1.0.0", "name": f"File {i}", "children": [],
                                   "pages": [{"id": "shared-page", "name": f"Canvas {i}", "children": []}]}))
    command = [str(args.binary.resolve()), "--serve-web", "--managed", "--port", "0",
               "--file", str(paths[0]), "--config-root", str(output / "config")]
    report = {"binary": str(args.binary.resolve()), "binarySha256": shared.sha(args.binary),
              "command": command, "logicalClients": clients, "files": args.files,
              "maxInflightRequests": min(args.max_inflight, clients),
              "expectedMutationRequests": clients * args.rounds,
              "expectedDrawingEdits": clients * args.rounds * args.batch,
              "oneSharedServiceProcess": True, "oneAuthorityPerFile": True,
              "externalLlmProcesses": 0, "agentContextsOptIn": args.agent_contexts,
              "globalUiIsolationExercised": False,
              "requestRetryPolicy": "none: ambiguous writes are never replayed"}
    process = sampler = None
    child_before = resource.getrusage(resource.RUSAGE_CHILDREN)
    all_started = time.perf_counter()
    log = (output / "server.stderr.log").open("wb")
    try:
        shared.phase("startup")
        process = await asyncio.create_subprocess_exec(*command, stdin=asyncio.subprocess.PIPE,
                  stdout=asyncio.subprocess.PIPE, stderr=log)
        sampler = asyncio.create_task(shared.monitor(process, report, args.max_bytes))
        handshake = json.loads(await asyncio.wait_for(process.stdout.readline(), 30))
        assert handshake.get("ok"), handshake
        port = handshake["port"]
        for i, path in enumerate(paths):
            empty = await shared.tool(port, f"prime-{i}", "read_nodes", {"filePath": str(path), "depth": "0"})
            assert empty["nodes"] == []

        expected = [set() for _ in paths]
        latencies = []
        inflight = peak_inflight = accepted = 0
        window = asyncio.Semaphore(args.max_inflight)

        async def client(identifier):
            nonlocal inflight, peak_inflight, accepted
            file_index = identifier % args.files
            for turn in range(args.rounds):
                nodes = []
                for index in range(args.batch):
                    name = f"client-{identifier}-turn-{turn}-node-{index}"
                    expected[file_index].add(name)
                    serial = (identifier * args.rounds + turn) * args.batch + index
                    nodes.append({"kind": "rect", "name": name, "width": 6, "height": 6,
                                  "x": (serial % 100) * 8, "y": (serial // 100) * 8,
                                  "fill_hex": COLORS[file_index]})
                async with window:
                    inflight += 1
                    peak_inflight = max(peak_inflight, inflight)
                    started = time.perf_counter()
                    try:
                        arguments = {"filePath": str(paths[file_index]), "nodes_json": json.dumps(nodes)}
                        if args.agent_contexts:
                            arguments["agentId"] = f"multi-client-{identifier}"
                        result = await shared.tool(port, f"{identifier}-{turn}", "batch_design", arguments)
                        assert str(result.get("wrote")).lower() == "true" and int(result.get("count", -1)) == args.batch, result
                        accepted += 1
                        latencies.append(time.perf_counter() - started)
                    finally:
                        inflight -= 1

        shared.phase("edit")
        started = time.perf_counter()
        await asyncio.gather(*(client(identifier) for identifier in range(clients)))
        report.update(editSeconds=time.perf_counter() - started, acceptedMutationRequests=accepted,
                      peakInflightRequests=peak_inflight,
                      requestLatencyP50Seconds=shared.percentile(latencies, .50),
                      requestLatencyP95Seconds=shared.percentile(latencies, .95))
        assert accepted == report["expectedMutationRequests"]
        shared.phase("verify")
        report["documents"] = []
        contexts = []
        for i, path in enumerate(paths):
            memory = await shared.tool(port, f"verify-{i}", "read_nodes", {"filePath": str(path), "depth": "0"})
            nodes = memory["nodes"]
            names = Counter(node["name"] for node in nodes)
            ids = [node["id"] for node in nodes]
            assert names == Counter({name: 1 for name in expected[i]}), f"lost/cross-file edits in file {i}"
            assert len(ids) == len(set(ids)), f"duplicate node IDs in file {i}"
            assert all(node["width"] == 6 and node["height"] == 6 for node in nodes)
            if i == 0:
                # The primary live authority persists through the normal explicit
                # Save route; alternate-file MCP writes persist before ack.
                primary = await shared.request(port, "GET", "/api/mcp/document")
                saved = await shared.request(port, "POST", "/api/file/save", {
                    "document": primary["document"], "activePageIndex": 0})
                assert saved.get("ok"), saved
            persisted = json.loads(path.read_text())
            durable_nodes = list(shared.nodes(persisted))
            assert Counter(node["name"] for node in durable_nodes) == names
            assert {node["id"] for node in durable_nodes} == set(ids)
            item = {"fileIndex": i, "nodes": len(nodes), "inMemoryExact": True,
                    "persistedExact": True, "persistedBytes": path.stat().st_size,
                    "persistedSha256": shared.sha(path)}
            if nodes:
                shared.phase("render")
                started = time.perf_counter()
                result = await shared.tool(port, f"render-{i}", "export_item", {
                    "filePath": str(path), "itemId": nodes[0]["id"], "format": "png", "scale": "1"})
                png = base64.b64decode(result["bytes_base64"], validate=True)
                assert pixels.center_pixel(png) == [int(COLORS[i][j:j + 2], 16) for j in [1, 3, 5]]
                png_path = output / f"file-{i}.png"
                png_path.write_bytes(png)
                item.update(renderSeconds=time.perf_counter() - started, pngSha256=shared.sha(png_path),
                            expectedColorVerified=True)
            report["documents"].append(item)
            if args.agent_contexts and len(nodes) >= 2:
                # Reuse the same two agents across every file. Alternating IDs
                # and zooms detect both agent and document context leakage.
                for agent in range(2):
                    context = {"filePath": str(path), "agentId": f"multi-client-{agent}"}
                    identifier = nodes[(i + agent) % 2]["id"]
                    zoom = 110 + i + agent * 100
                    await shared.tool(port, f"select-{i}-{agent}", "set_selection", {**context, "node_id": identifier})
                    await shared.tool(port, f"zoom-{i}-{agent}", "set_viewport", {**context, "zoom_percent": str(zoom)})
                    contexts.append((i, agent, context, identifier, zoom))
        for i, agent, context, identifier, zoom in contexts:
            selection = await shared.tool(port, f"selection-{i}-{agent}", "get_selection_set", context)
            viewport = await shared.tool(port, f"viewport-{i}-{agent}", "get_viewport", context)
            assert selection["anchor"] == identifier and selection["ids"] == identifier, selection
            assert int(viewport["zoom_percent"]) == zoom, viewport
        if args.agent_contexts:
            assert contexts, "context workload requires nonempty files"
            assert all(shared.sha(path) == item["persistedSha256"] for path, item in zip(paths, report["documents"]))
            report.update(globalUiIsolationExercised=True, agentDocumentContextsVerified=len(contexts),
                          uiOnlyChangesPreservedPersistedBytes=True)
        report["correctness"] = True
    except BaseException as error:
        report.update(correctness=False, error=f"{type(error).__name__}: {error}")
        raise
    finally:
        if process:
            if process.stdin:
                process.stdin.close()
            try:
                await asyncio.wait_for(process.wait(), 10)
            except asyncio.TimeoutError:
                process.kill()
                await process.wait()
            report["serverExitCode"] = process.returncode
        if sampler:
            await sampler
        log.close()
        cpu_after = resource.getrusage(resource.RUSAGE_CHILDREN)
        report.update(totalWallSeconds=time.perf_counter() - all_started,
                      serviceCpuSeconds=cpu_after.ru_utime + cpu_after.ru_stime - child_before.ru_utime - child_before.ru_stime)
        report["correctness"] = report.get("correctness", False) and report.get("serverExitCode") == 0
        report["correctness"] &= not report.get("memoryBudgetExceeded", False)
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report), flush=True)
    return report


async def suite(args):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    reports = []
    for count in args.clients:
        try:
            reports.append(await asyncio.wait_for(benchmark(args, count, output / str(count)), args.timeout))
        except Exception as error:
            print(f"{type(error).__name__}: {error}", file=sys.stderr)
            return False
        (output / "summary.json").write_text(json.dumps({"results": reports}, indent=2) + "\n")
        if not reports[-1]["correctness"]:
            return False
    return True


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--clients", type=int, nargs="+", default=[10, 100, 1000])
    parser.add_argument("--files", type=int, default=3)
    parser.add_argument("--max-inflight", type=int, default=32)
    parser.add_argument("--rounds", type=int, default=2)
    parser.add_argument("--batch", type=int, default=4)
    parser.add_argument("--agent-contexts", action="store_true", help="use distinct agentId values and verify UI isolation across files")
    parser.add_argument("--max-bytes", type=int, default=1_000_000_000)
    parser.add_argument("--timeout", type=float, default=90)
    args = parser.parse_args()
    if not (1 <= args.files <= 8 and 1 <= args.max_inflight <= 48 and 1 <= args.rounds <= 100
            and 1 <= args.batch <= 64 and all(1 <= n <= 10000 for n in args.clients)
            and args.max_bytes > 0 and args.timeout > 0):
        parser.error("bounded positive workloads required")
    sys.exit(0 if asyncio.run(suite(args)) else 1)
