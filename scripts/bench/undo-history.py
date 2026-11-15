#!/usr/bin/env python3
"""Guard actual MCP standalone edits, durable undo/redo and rendering on a large frame.

Uses a resident alternate file so every acknowledgement follows its atomic save.
One logical client/request is active; no external model processes are launched.
"""
import argparse
import asyncio
import base64
import hashlib
import importlib.util
import json
from pathlib import Path
import resource
import struct
import time

spec = importlib.util.spec_from_file_location("shared", Path(__file__).with_name("headless-agents.py"))
shared = importlib.util.module_from_spec(spec)
spec.loader.exec_module(shared)


def fixture(leaves):
    return {"version": "1.0.0", "editorMeta": {"preserveAuthoredGeometry": True},
            "children": [{"type": "frame", "id": "board", "width": 1000, "height": 1000,
                          "children": [{"type": "rectangle", "id": f"leaf-{i}",
                                        "x": (i % 100) * 10, "y": (i // 100) * 10,
                                        "width": 6, "height": 6,
                                        "fill": [{"type": "solid", "color": "#2563eb"}]}
                                       for i in range(leaves)]}]}


async def run(args):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    primary, document = output / "primary.op", output / "large.op"
    primary.write_text('{"version":"1.0.0","children":[]}')
    expected = fixture(args.leaves)
    document.write_text(json.dumps(expected, separators=(",", ":")))
    expected.pop("editorMeta")
    target = expected["children"][0]["children"][-1]
    original_x = target["x"]
    # Keep every authored edit inside the board and different from the original.
    edit_base = original_x + 1 if original_x < 500 else original_x - 101
    command = [str(args.binary.resolve()), "--serve-web", "--managed", "--port", "0",
               "--file", str(primary), "--config-root", str(output / "config")]
    report = {"binarySha256": shared.sha(args.binary), "command": command,
              "leafNodes": args.leaves, "edits": args.edits, "logicalClients": 1,
              "maxInflightRequests": 1, "requestRetryPolicy": "none", "samples": {},
              "invariants": {}, "status": "running"}
    log = (output / "server.stderr.log").open("wb")
    process = sampler = None
    started, cpu_before = time.perf_counter(), resource.getrusage(resource.RUSAGE_CHILDREN)
    sequence = 0
    try:
        shared.phase("startup")
        process = await asyncio.create_subprocess_exec(*command, stdin=asyncio.subprocess.PIPE,
                  stdout=asyncio.subprocess.PIPE, stderr=log)
        sampler = asyncio.create_task(shared.monitor(process, report, args.max_bytes))
        handshake = json.loads(await asyncio.wait_for(process.stdout.readline(), 30))
        assert handshake.get("ok"), handshake
        port = handshake["port"]

        async def call(phase, tool, extra):
            nonlocal sequence
            sequence += 1
            shared.phase(phase)
            begin = time.perf_counter()
            result = await shared.tool(port, sequence, tool, {"filePath": str(document), **extra})
            report["samples"].setdefault(phase, []).append(time.perf_counter() - begin)
            return result

        def check_disk(phase, step):
            # Editor metadata is outside the design document; every other key,
            # value and None-versus-empty child distinction is compared.
            actual = json.loads(document.read_text())
            actual.pop("editorMeta", None)
            assert actual == expected, f"durable canonical document differs at {phase}/{step}"

        async def raster(label):
            result = await call(label, "export_item", {"itemId": "board", "format": "png", "scale": "1"})
            data = base64.b64decode(result["bytes_base64"], validate=True)
            assert data.startswith(b"\x89PNG\r\n\x1a\n")
            assert struct.unpack(">II", data[16:24]) == (1000, 1000)
            (output / f"{label}.png").write_bytes(data)
            return hashlib.sha256(data).hexdigest()

        original_png = await raster("original")
        check_disk("original", 0)
        for i in range(args.edits):
            await call("edit", "update_node", {"node_id": target["id"], "x": str(edit_base + i)})
            target["x"] = edit_base + i
            check_disk("edit", i)
        edited_png = await raster("edited")
        assert edited_png != original_png
        report["invariants"]["everyEditDurableAndUnrelatedDataExact"] = True
        for i in range(args.edits):
            await call("undo", "undo", {})
            target["x"] = original_x if i == args.edits - 1 else edit_base + args.edits - 2 - i
            check_disk("undo", i)
        assert await raster("undone") == original_png
        report["invariants"]["allUndoDurableAndOriginalPngExact"] = True
        for i in range(args.edits):
            await call("redo", "redo", {})
            target["x"] = edit_base + i
            check_disk("redo", i)
        assert await raster("redone") == edited_png
        report["invariants"]["allRedoDurableAndEditedPngExact"] = True
        report.update(status="completed", completedRequests=sequence, finalSha256=shared.sha(document))
    except BaseException as error:
        report.update(status="failed", error=f"{type(error).__name__}: {error}", completedRequests=sequence)
    finally:
        if process:
            process.stdin.close()
            try:
                await asyncio.wait_for(process.wait(), 10)
            except asyncio.TimeoutError:
                process.terminate()
                await process.wait()
        if sampler:
            await sampler
        log.close()
        cpu_after = resource.getrusage(resource.RUSAGE_CHILDREN)
        report["serviceCpuSeconds"] = cpu_after.ru_utime + cpu_after.ru_stime - cpu_before.ru_utime - cpu_before.ru_stime
        report["wallSeconds"] = time.perf_counter() - started
        report["serverExitCode"] = process.returncode if process else None
        if report.get("memoryBudgetExceeded") or report["serverExitCode"] != 0:
            report["status"] = "failed"
        report["latency"] = {phase: {"count": len(values), "p50Seconds": shared.percentile(values, .5),
                                     "p95Seconds": shared.percentile(values, .95)}
                             for phase, values in report["samples"].items()}
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({k: v for k, v in report.items() if k != "samples"}, indent=2), flush=True)
    return report["status"] == "completed"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--leaves", type=int, default=10000)
    parser.add_argument("--edits", type=int, default=100)
    parser.add_argument("--max-bytes", type=int, default=1_000_000_000)
    parser.add_argument("--timeout", type=float, default=180)
    args = parser.parse_args()
    assert 100 <= args.leaves <= 10000 and 1 <= args.edits <= 100
    raise SystemExit(0 if asyncio.run(asyncio.wait_for(run(args), args.timeout)) else 1)


if __name__ == "__main__":
    main()
