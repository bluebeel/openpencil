#!/usr/bin/env python3
"""Measure real MCP bulk exports and verify every requested file and its format.

The generated input streams to disk. No user document is opened or changed.
Compare two runs with --baseline to check exact file bytes as well as timing.
"""
import argparse
import asyncio
import hashlib
import importlib.util
import json
from pathlib import Path
import resource
import struct
import time

spec = importlib.util.spec_from_file_location("shared_benchmark", Path(__file__).with_name("headless-agents.py"))
shared = importlib.util.module_from_spec(spec)
spec.loader.exec_module(shared)


def fixture(path, frames, children):
    with path.open("w") as stream:
        stream.write('{"version":"1.0.0","children":[')
        for frame in range(frames):
            if frame:
                stream.write(',')
            node = {"id": f"f{frame}", "type": "frame", "name": f"Board-{frame:04d}",
                    "x": frame * 200, "width": 160, "height": 160,
                    "fill": [{"type": "solid", "color": "#e2e8f0"}],
                    "children": [{"id": f"f{frame}-r{i}", "type": "rectangle",
                        "x": i % 10 * 14, "y": i // 10 * 14, "width": 10, "height": 10,
                        "fill": [{"type": "solid", "color": "#2563eb"}]} for i in range(children)]}
            stream.write(json.dumps(node, separators=(",", ":")))
        stream.write(']}')


async def run(args):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    document = output / "boards.op"
    fixture(document, args.frames, args.children)
    command = [str(args.binary.resolve()), "--serve-web", "--managed", "--port", "0",
               "--file", str(document), "--config-root", str(output / "config")]
    report = {"binarySha256": shared.sha(args.binary), "command": command, "frames": args.frames,
              "childrenPerFrame": args.children, "nodes": args.frames * (args.children + 1),
              "format": args.format, "invariants": {}, "requestRetryPolicy": "none", "files": []}
    report["goalBudgetBytes"] = 1_000_000_000
    report["diagnosticBudgetAboveGoal"] = args.max_bytes > report["goalBudgetBytes"]
    process = sampler = None
    log = (output / "server.stderr.log").open("wb")
    before_child = resource.getrusage(resource.RUSAGE_CHILDREN)
    all_start = time.perf_counter()
    try:
        shared.phase("startup")
        process = await asyncio.create_subprocess_exec(*command, stdin=asyncio.subprocess.PIPE,
                  stdout=asyncio.subprocess.PIPE, stderr=log)
        sampler = asyncio.create_task(shared.monitor(process, report, args.max_bytes, args.memory_scope))
        handshake = json.loads(await asyncio.wait_for(process.stdout.readline(), 30))
        assert handshake.get("ok"), handshake
        port = handshake["port"]
        original_hash = shared.sha(document)
        shared.phase("bulk-export")
        before = shared.usage(process.pid)
        start = time.perf_counter()
        result = await shared.tool(port, "bulk-export", "export_frames",
                 {"outputDir": str(output / "images"), "format": args.format})
        report["exportSeconds"] = time.perf_counter() - start
        after = shared.usage(process.pid)
        report["exportCpuSeconds"] = after["cpuSeconds"] - before["cpuSeconds"] if before and after else None
        assert result["attempted"] == args.frames and not result["failed"], result
        assert len(result["written"]) == len(set(result["written"])) == args.frames
        shared.phase("verify")
        for name in result["written"]:
            data = (output / "images" / name).read_bytes()
            if args.format == "png":
                assert data.startswith(b"\x89PNG\r\n\x1a\n") and struct.unpack(">II", data[16:24]) == (320, 320)
            elif args.format in ("jpeg", "jpg"):
                assert data.startswith(b"\xff\xd8\xff")
            else:
                assert data[:4] == b"RIFF" and data[8:12] == b"WEBP"
            assert name.endswith("." + args.format), name
            report["files"].append({"name": name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
        report["invariants"].update(allFramesWritten=True, uniqueNames=True, correctFormats=True,
                                      originalDocumentUnchanged=shared.sha(document) == original_hash)
        if args.baseline:
            baseline = json.loads((args.baseline / "report.json").read_text())
            assert (baseline["frames"], baseline["childrenPerFrame"], baseline["format"]) == (args.frames, args.children, args.format)
            report["invariants"]["exactFilesMatchBaseline"] = report["files"] == baseline["files"]
        assert all(report["invariants"].values()), report["invariants"]
        report["status"] = "completed"
    except Exception as error:
        report.update(status="failed", error=repr(error))
    finally:
        if process and process.returncode is None:
            process.stdin.close()
            try:
                await asyncio.wait_for(process.wait(), 10)
            except asyncio.TimeoutError:
                process.terminate()
                await process.wait()
        if sampler:
            await sampler
        log.close()
        after_child = resource.getrusage(resource.RUSAGE_CHILDREN)
        report["serviceCpuSeconds"] = after_child.ru_utime + after_child.ru_stime - before_child.ru_utime - before_child.ru_stime
        report["wallSeconds"] = time.perf_counter() - all_start
        report["exitCode"] = process.returncode if process else None
        if report.get("memoryBudgetExceeded") or report["exitCode"] != 0:
            report["status"] = "failed"
        prefix = "Service" if args.memory_scope == "service" else "Aggregate"
        measured = [report.get(f"peak{prefix}RssBytes"),
                    report.get(f"peak{prefix}PhysicalFootprintBytes"),
                    report.get("kernelChildMaxRssBytes")]
        report["measuredScopeWithinGoalBudget"] = (report["status"] == "completed"
            and report.get("kernelChildMaxRssBytes") is not None
            and all(value <= report["goalBudgetBytes"] for value in measured if value is not None))
        (output / "report.json").write_text(json.dumps(report, indent=2))
    print(json.dumps({key: value for key, value in report.items() if key != "files"}, indent=2), flush=True)
    return report["status"] == "completed"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--frames", type=int, default=100)
    parser.add_argument("--children", type=int, default=100)
    parser.add_argument("--format", choices=["png", "jpeg", "jpg", "webp"], default="png")
    parser.add_argument("--max-bytes", type=int, default=1_000_000_000)
    parser.add_argument("--memory-scope", choices=["aggregate", "service"], default="aggregate",
                        help="charge both service and generator, or only the shared service/renderer")
    args = parser.parse_args()
    if not (1 <= args.frames <= 1000 and 0 <= args.children <= 100 and args.max_bytes > 0):
        parser.error("frames must be 1..1000, children 0..100, and budget positive")
    raise SystemExit(0 if asyncio.run(run(args)) else 1)


if __name__ == "__main__":
    main()
