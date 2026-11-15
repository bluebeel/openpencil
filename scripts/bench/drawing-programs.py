#!/usr/bin/env python3
"""Measure atomic MCP drawing programs, exact durable state and undo/redo.

Each program copies a subtree, moves it, removes it, then makes a retained
leaf edit. A real resident file saves every accepted program/undo/redo.
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
    template = {"type":"frame", "id":"template", "name":"Copy source",
        "width":64,"height":64,"children":[{"type":"rectangle","id":f"template-{i}",
        "x":(i%8)*8,"y":(i//8)*8,"width":6,"height":6,
        "fill":[{"type":"solid","color":"#ff6600"}]} for i in range(args.template_leaves)]}
    for level in range(args.template_depth):
        template["id"] = f"template-level-{level}"
        template = {"type":"frame", "id":"template", "width":64, "height":64,
                    "children":[template]}
    expected["children"].append(template)
    document.write_text(json.dumps(expected, separators=(",", ":")))
    expected.pop("editorMeta")
    target = expected["children"][0]["children"][-1]
    original_x = target["x"]
    # Keep every authored edit inside the board and different from the original.
    edit_base = original_x + 1 if original_x < 500 else original_x - 101
    command = [str(args.binary.resolve()), "--serve-web", "--managed", "--port", "0",
               "--file", str(primary), "--config-root", str(output / "config")]
    report = {"binarySha256": shared.sha(args.binary), "command": command,
              "leafNodes": args.leaves, "rounds": args.rounds, "logicalClients": 1,
              "copiesPerProgram":args.copies,"templateDepth":args.template_depth,
              "nodesPerCopiedSubtree":args.template_leaves+args.template_depth+1,
              "commandsPerProgram":args.copies*3+1,
              "initialNodes":args.leaves+args.template_leaves+args.template_depth+2,
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
            cpu_before = shared.usage(process.pid)
            result = await shared.tool(port, sequence, tool, {"filePath": str(document), **extra})
            report["samples"].setdefault(phase, []).append(time.perf_counter() - begin)
            cpu_after = shared.usage(process.pid)
            if cpu_before and cpu_after:
                phases=report.setdefault("phaseServiceCpuSeconds",{})
                phases[phase]=phases.get(phase,0)+cpu_after["cpuSeconds"]-cpu_before["cpuSeconds"]
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
        for i in range(args.rounds):
            operations=[]
            for copy in range(args.copies):
                operations += [f'copy{copy}=C("template", null, '+json.dumps({"name":f"Temporary {copy}"})+')',
                    f'U(copy{copy}, '+json.dumps({"x":copy,"y":copy})+')', f'D(copy{copy})']
            operations.append('U('+json.dumps(target["id"])+', '+json.dumps({"x":edit_base+i})+')')
            result=await call("program", "batch_design", {"operations":";\n".join(operations)})
            assert not result.get("errors") and result.get("applied") is not False, result
            assert len(result["results"])==args.copies and result["nodeCount"]==report["initialNodes"], result
            target["x"] = edit_base + i
            check_disk("program", i)
        edited_png = await raster("edited")
        assert edited_png != original_png
        report["invariants"]["everyProgramDurableAndUnrelatedDataExact"] = True
        for i in range(args.rounds):
            await call("undo", "undo", {})
            target["x"] = original_x if i == args.rounds - 1 else edit_base + args.rounds - 2 - i
            check_disk("undo", i)
        assert await raster("undone") == original_png
        report["invariants"]["allUndoDurableAndOriginalPngExact"] = True
        for i in range(args.rounds):
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
    parser.add_argument("--rounds", type=int, default=10)
    parser.add_argument("--copies", type=int, default=100)
    parser.add_argument("--template-leaves", type=int, default=32)
    parser.add_argument("--template-depth", type=int, default=0)
    parser.add_argument("--max-bytes", type=int, default=1_000_000_000)
    parser.add_argument("--timeout", type=float, default=180)
    args = parser.parse_args()
    assert 0 <= args.template_depth <= 32
    assert 1 <= args.copies <= 100 and 1 <= args.template_leaves <= 100
    assert 100 <= args.leaves <= 10000 and 1 <= args.rounds <= 100
    raise SystemExit(0 if asyncio.run(asyncio.wait_for(run(args), args.timeout)) else 1)


if __name__ == "__main__":
    main()
