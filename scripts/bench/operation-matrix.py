#!/usr/bin/env python3
"""Production MCP operation latency/semantic matrix on a synthetic large document.

Uses one actual serve-web process. The initial matrix is serial so operation
latency and global selection/clipboard/history semantics remain distinguishable
from contention. The separate headless-agents.py benchmark supplies concurrent
10/100/1000-client coverage. No design corpus or external image/font assets.
"""
import argparse
import asyncio
import base64
from collections import Counter
import hashlib
import importlib.util
import json
from pathlib import Path
import resource
import struct
import sys
import time
import zlib

spec = importlib.util.spec_from_file_location("shared_benchmark", Path(__file__).with_name("headless-agents.py"))
shared = importlib.util.module_from_spec(spec)
spec.loader.exec_module(shared)


def png(width, height, rgba):
    def chunk(kind, value):
        return struct.pack(">I", len(value)) + kind + value + struct.pack(">I", zlib.crc32(kind + value))
    rows = b"".join(b"\0" + rgba[y * width * 4:(y + 1) * width * 4] for y in range(height))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


def center_pixel(encoded):
    cursor, compressed = 8, bytearray()
    while cursor < len(encoded):
        length = int.from_bytes(encoded[cursor:cursor + 4], "big")
        kind, value = encoded[cursor + 4:cursor + 8], encoded[cursor + 8:cursor + 8 + length]
        if kind == b"IHDR":
            width, height, depth, color, _, _, interlace = struct.unpack(">IIBBBBB", value)
            assert depth == 8 and color in (2, 6) and interlace == 0
        elif kind == b"IDAT":
            compressed.extend(value)
        cursor += length + 12
    channels = 4 if color == 6 else 3
    stride = width * channels
    raw = zlib.decompress(compressed)
    previous = bytearray(stride)
    for y in range(height // 2 + 1):
        offset = y * (stride + 1)
        mode, row = raw[offset], bytearray(raw[offset + 1:offset + 1 + stride])
        for i in range(stride):
            left, above = row[i - channels] if i >= channels else 0, previous[i]
            upper = previous[i - channels] if i >= channels else 0
            estimate = left + above - upper
            distances = [abs(estimate - left), abs(estimate - above), abs(estimate - upper)]
            predictor = [0, left, above, (left + above) // 2, [left, above, upper][distances.index(min(distances))]][mode]
            row[i] = (row[i] + predictor) & 255
        previous = row
    offset = (width // 2) * channels
    return list(previous[offset:offset + 3])


def fixture(count, nested):
    reserved = 6 if nested else 5
    children = [{"type": "rectangle", "id": f"n{i}", "name": f"rectangle-{i}",
                 "x": ((i - 1) % 100) * 8, "y": ((i - 1) // 100) * 8,
                 "width": 6, "height": 6, "fill": [{"type": "solid", "color": "#2563eb"}]}
                for i in range(1, count - reserved + 1)]
    component = f"n{count - 4}"
    children.append({"type": "frame", "id": component, "name": "Component prototype", "x": 600, "y": 840,
                     "width": 32, "height": 32, "children": [
                         {"type": "rectangle", "id": f"n{count - 3}", "width": 16, "height": 32,
                          "fill": [{"type": "solid", "color": "#ff0000"}]},
                         {"type": "rectangle", "id": f"n{count - 2}", "x": 16, "width": 16, "height": 32,
                          "fill": [{"type": "solid", "color": "#00ff00"}]}]})
    children.append({"type": "text", "id": f"n{count - 1}", "name": "Text control", "x": 20, "y": 840,
                     "width": 400, "height": 28, "content": "Shared text", "fontSize": 18,
                     "fontFamily": "Roboto", "fontWeight": 400, "fontKerning": True,
                     "fill": [{"type": "solid", "color": "#222222"}]})
    image = png(4, 4, bytes([31, 161, 97, 255]) * 16)
    children.append({"type": "rectangle", "id": f"n{count}", "name": "Image control", "x": 450, "y": 840,
                     "width": 32, "height": 32, "fill": [{"type": "image", "nearestSampling": True,
                         "url": "data:image/png;base64," + base64.b64encode(image).decode()}]})
    if nested:
        children = [{"type": "frame", "id": "board", "width": 1100, "height": 900, "children": children}]
    result = {"version": "1.0.0", "children": [], "pages": [{"id": "matrix-page", "name": "Matrix", "children": children}]}
    assert len(list(shared.nodes(result))) == count
    return result


async def matrix(args):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    document_path = output / "matrix.op"
    initial = fixture(args.nodes, args.topology == "nested")
    document_path.write_text(json.dumps(initial, separators=(",", ":")))
    command = [str(args.binary.resolve()), "--serve-web", "--managed", "--port", "0", "--file", str(document_path),
               "--config-root", str(output / "config")]
    report = {"binary": str(args.binary.resolve()), "binarySha256": shared.sha(args.binary), "command": command,
              "initialNodes": args.nodes, "topology": args.topology, "logicalClients": 1, "maxInflightRequests": 1,
              "iterations": args.iterations, "externalAssets": False, "agentContextsOptIn": args.agent_contexts, "operationSamples": {}, "invariants": {},
              "note": "Serial per-operation latency. Concurrent shared-state insert coverage is in headless-agents.py."}
    process = sampler = None
    stderr = (output / "server.stderr.log").open("wb")
    started_all = time.perf_counter()
    child_start = resource.getrusage(resource.RUSAGE_CHILDREN)
    try:
        shared.phase("startup")
        process = await asyncio.create_subprocess_exec(*command, stdin=asyncio.subprocess.PIPE,
                  stdout=asyncio.subprocess.PIPE, stderr=stderr)
        sampler = asyncio.create_task(shared.monitor(process, report, args.max_bytes))
        handshake = json.loads(await asyncio.wait_for(process.stdout.readline(), 30))
        assert handshake.get("ok"), handshake
        port = handshake["port"]
        request_id = 0

        async def call(label, tool, arguments):
            nonlocal request_id
            request_id += 1
            shared.phase(label)
            before = shared.usage(process.pid)
            started = time.perf_counter()
            if args.agent_contexts:
                agent = "matrix-B" if label.startswith("context-B") else "matrix-A" if label.startswith("context-A") else "matrix-main"
                arguments = {**arguments, "agentId": agent}
            result = await shared.tool(port, f"matrix-{request_id}", tool, arguments)
            elapsed = time.perf_counter() - started
            after = shared.usage(process.pid)
            report["operationSamples"].setdefault(label, []).append({"wallSeconds": elapsed,
                "serviceCpuSeconds": after["cpuSeconds"] - before["cpuSeconds"] if before and after else None})
            return result

        async def document():
            response = await shared.request(port, "GET", "/api/mcp/document")
            all_nodes = list(shared.nodes(response["document"]))
            assert len({node["id"] for node in all_nodes}) == len(all_nodes)
            return response, {node["id"]: node for node in all_nodes}

        async def raster(label, identifier):
            result = await call(label, "export_item", {"itemId": identifier, "format": "png", "scale": "1"})
            encoded = base64.b64decode(result["bytes_base64"], validate=True)
            assert encoded[:8] == b"\x89PNG\r\n\x1a\n"
            (output / f"{label}-{request_id}.png").write_bytes(encoded)
            return encoded

        _, baseline = await document()
        assert len(baseline) == args.nodes
        for i in range(args.iterations):
            await call("move", "update_node", {"node_id": "n1", "x": str(100 + i), "y": str(120 + i)})
        _, state = await document()
        assert state["n1"]["x"] == 99 + args.iterations and state["n1"]["y"] == 119 + args.iterations
        report["invariants"]["move"] = True
        for i in range(args.iterations):
            await call("resize", "update_node", {"node_id": "n2", "width": str(16 + i), "height": str(18 + i)})
        _, state = await document()
        assert state["n2"]["width"] == 15 + args.iterations and state["n2"]["height"] == 17 + args.iterations
        report["invariants"]["resize"] = True
        for i in range(args.iterations):
            await call("duplicate", "copy_node", {"node_id": "n3", "parent": "board" if args.topology == "nested" else "",
                                                   "overrides": json.dumps({"name": f"duplicate-{i}"})})
            _, state = await document()
            added = set(state) - set(baseline)
            assert len(added) == 1 and state[next(iter(added))]["name"] == f"duplicate-{i}"
            await call("cleanup", "delete_node", {"node_id": next(iter(added))})
        report["invariants"]["duplicate"] = True
        text_id, image_id, component_id = f"n{args.nodes - 1}", f"n{args.nodes}", f"n{args.nodes - 4}"
        for i in range(args.iterations):
            await call("text", "set_node_text", {"node_id": text_id, "text": f"Shared text {i}"})
        _, state = await document()
        assert state[text_id]["content"] == f"Shared text {args.iterations - 1}"
        report["invariants"]["text"] = True
        for _ in range(args.iterations):
            await call("shadow", "add_node_effect", {"node_id": "n4", "kind": "shadow"})
            _, state = await document()
            assert len(state["n4"]["effects"]) == 1
            await call("cleanup", "remove_node_effect", {"node_id": "n4", "index": "0"})
        report["invariants"]["shadow"] = True
        await call("component-create", "create_component", {"node_id": component_id, "name": "Matrix component"})
        for _ in range(args.iterations):
            await call("component-instance", "instantiate_component", {"component_id": component_id})
            response, state = await document()
            added = set(state) - set(baseline)
            assert len(added) == 3
            roots = response["document"]["pages"][0]["children"]
            created = next(node for node in roots if node["id"] in added)
            await call("cleanup", "delete_node", {"node_id": created["id"]})
        report["invariants"]["component"] = True
        svg = '<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><circle cx="8" cy="8" r="8" fill="#00aa55"/></svg>'
        await call("svg-import", "import_svg", {"svg": svg, "x": "900", "y": "840"})
        response, state = await document()
        added = set(state) - set(baseline)
        assert added
        created = next(node for node in response["document"]["pages"][0]["children"] if node["id"] in added)
        encoded = await raster("svg-export", created["id"])
        assert center_pixel(encoded) == [0, 170, 85], center_pixel(encoded)
        await call("cleanup", "delete_node", {"node_id": created["id"]})
        report["invariants"]["svgPixels"] = True
        encoded = await raster("image-export", image_id)
        assert center_pixel(encoded) == [31, 161, 97], center_pixel(encoded)
        report["invariants"]["imagePixels"] = True
        await call("selection", "set_selection", {"node_id": "n5"})
        await call("copy", "copy_selected", {})
        await call("paste", "paste_clipboard", {"offset_px": "0"})
        _, state = await document()
        added = set(state) - set(baseline)
        assert len(added) == 1 and state[next(iter(added))]["name"] == baseline["n5"]["name"]
        await call("cleanup", "delete_node", {"node_id": next(iter(added))})
        report["invariants"]["copyPaste"] = True
        for i in range(args.iterations):
            await call("zoom", "set_viewport", {"zoom_percent": str(125 + i)})
        viewport = await call("zoom-read", "get_viewport", {})
        assert int(viewport["zoom_percent"]) == 124 + args.iterations
        report["invariants"]["zoom"] = True
        original = await raster("undo-before", "n6")
        await call("fill", "set_node_fill_hex", {"node_id": "n6", "hex": "#ff0000"})
        changed = await raster("undo-changed", "n6")
        await call("undo", "undo", {})
        restored = await raster("undo-restored", "n6")
        await call("redo", "redo", {})
        replayed = await raster("undo-replayed", "n6")
        assert original == restored and changed == replayed and original != changed
        report["invariants"]["undoRedoPixels"] = True
        # Deterministic two-agent interleaving, never a timing-dependent race:
        # A selects n7, B selects n8, then A copies/pastes and reads viewport.
        await call("context-A-select", "set_selection", {"node_id": "n7"})
        await call("context-B-select", "set_selection", {"node_id": "n8"})
        await call("context-A-copy", "copy_selected", {})
        await call("context-A-paste", "paste_clipboard", {"offset_px": "0"})
        _, state = await document()
        added = set(state) - set(baseline)
        assert len(added) == 1
        copied = state[next(iter(added))]
        await call("context-A-zoom", "set_viewport", {"zoom_percent": "125"})
        await call("context-B-zoom", "set_viewport", {"zoom_percent": "200"})
        viewport = await call("context-A-read", "get_viewport", {})
        report["globalUiContextGap"] = {"logicalClients": 2, "sequence": "Aselect7,Bselect8,Acopy,Apaste,Azoom125,Bzoom200,Aread",
            "agentAExpectedCopiedName": baseline["n7"]["name"], "agentAObservedCopiedName": copied["name"],
            "agentAExpectedZoom": 125, "agentAObservedZoom": int(viewport["zoom_percent"]),
            "isolated": copied["name"] == baseline["n7"]["name"] and int(viewport["zoom_percent"]) == 125,
            "scope": "Current service has one global MCP UI context; logical agent labels do not create isolated sessions."}
        if args.agent_contexts:
            assert report["globalUiContextGap"]["isolated"], report["globalUiContextGap"]
            report["globalUiContextGap"]["scope"] = "Opt-in agentId isolates UI context; document and undo history remain shared."
            report["invariants"]["agentContextIsolation"] = True
            second_path = output / "clipboard-destination.op"
            second_path.write_text(json.dumps({"version": "1.0.0", "children": [], "pages": [
                {"id": "destination", "name": "Destination", "children": []}]}))
            await call("context-A-cross-file-paste", "paste_clipboard", {"offset_px": "0", "filePath": str(second_path)})
            pasted = list(shared.nodes(json.loads(second_path.read_text())))
            assert len(pasted) == 1 and pasted[0]["name"] == baseline["n7"]["name"]
            report["invariants"]["crossFileAgentClipboard"] = True
        await call("cleanup", "delete_node", {"node_id": next(iter(added))})
        response, state = await document()
        assert len(state) == args.nodes and set(state) == set(baseline)
        shared.phase("persist")
        saved = await shared.request(port, "POST", "/api/file/save", {"document": response["document"], "activePageIndex": 0})
        assert saved.get("ok")
        persisted = list(shared.nodes(json.loads(document_path.read_text())))
        assert Counter(node["id"] for node in persisted) == Counter(state.keys())
        report.update(correctness=True, finalNodes=len(state), persistedSha256=shared.sha(document_path))
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
                process.terminate()
                await asyncio.wait_for(process.wait(), 5)
            report["serverExitCode"] = process.returncode
        if sampler:
            await sampler
        stderr.close()
        child_end = resource.getrusage(resource.RUSAGE_CHILDREN)
        report["serviceCpuSeconds"] = child_end.ru_utime + child_end.ru_stime - child_start.ru_utime - child_start.ru_stime
        report["totalWallSeconds"] = time.perf_counter() - started_all
        report["operations"] = {label: {"samples": len(samples),
            "wallP50Seconds": shared.percentile([sample["wallSeconds"] for sample in samples], .5),
            "wallP95Seconds": shared.percentile([sample["wallSeconds"] for sample in samples], .95),
            "serviceCpuSeconds": sum(sample["serviceCpuSeconds"] or 0 for sample in samples)}
            for label, samples in report["operationSamples"].items()}
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--nodes", type=int, default=10000)
    parser.add_argument("--iterations", type=int, default=3)
    parser.add_argument("--topology", choices=["flat", "nested"], default="flat")
    parser.add_argument("--agent-contexts", action="store_true", help="verify isolated agentId selection/clipboard/viewport and cross-file paste")
    parser.add_argument("--max-bytes", type=int, default=1_000_000_000)
    parser.add_argument("--timeout", type=float, default=90)
    args = parser.parse_args()
    assert 16 <= args.nodes <= 100000 and 1 <= args.iterations <= 100
    asyncio.run(asyncio.wait_for(matrix(args), args.timeout))


if __name__ == "__main__":
    main()
