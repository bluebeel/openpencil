#!/usr/bin/env python3
"""Benchmark real shared headless MCP edits, raster export, and durable save.

Build: cargo build -p op-host-web-server
Run: python3 scripts/bench/headless-agents.py --binary target/debug/op-host-web-server \
     --clients 10 100 1000 --max-inflight 32 --output /tmp/headless-agent-benchmark

Each logical client issues sequential batches against ONE document authority;
only the request window is bounded. External LLM/model processes are absent.
macOS reports both RSS and physical footprint; Linux reports RSS. CPU covers
one shared service (including its renderer) and this load generator separately.
"""
import argparse
import asyncio
import base64
import ctypes
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import resource
import struct
import sys
import time


CURRENT_PHASE = "startup"


class DarwinUsage(ctypes.Structure):
    _fields_ = [("uuid", ctypes.c_uint8 * 16)] + [(name, ctypes.c_uint64) for name in [
        "user_time", "system_time", "pkg_idle_wkups", "interrupt_wkups", "pageins",
        "wired_size", "resident_size", "phys_footprint", "proc_start_abstime", "proc_exit_abstime",
        "child_user_time", "child_system_time", "child_pkg_idle_wkups", "child_interrupt_wkups",
        "child_pageins", "child_elapsed_abstime", "diskio_bytesread", "diskio_byteswritten",
    ]]


USAGE_API = None
if sys.platform == "darwin":
    USAGE_API = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True).proc_pid_rusage
    USAGE_API.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_void_p]
    USAGE_API.restype = ctypes.c_int


def usage(pid):
    if USAGE_API:
        info = DarwinUsage()
        if USAGE_API(pid, 2, ctypes.byref(info)) == 0:
            return {"rss": info.resident_size, "footprint": info.phys_footprint,
                    "cpuSeconds": (info.user_time + info.system_time) / 1e9}
        return None
    try:
        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        resident = int(Path(f"/proc/{pid}/statm").read_text().split()[1]) * os.sysconf("SC_PAGE_SIZE")
        return {"rss": resident, "footprint": None,
                "cpuSeconds": (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")}
    except (OSError, ValueError, IndexError):
        return None


async def monitor(process, report, limit):
    report["memoryLimitBytes"] = limit
    report["memorySampleIntervalSeconds"] = .05
    report["memoryScope"] = "shared service/renderer process + load-generator process; no external LLMs"
    report["physicalFootprintAvailable"] = bool(USAGE_API)
    report["peakAggregateRssBytes"] = report["peakServiceRssBytes"] = 0
    report["peakAggregatePhysicalFootprintBytes"] = report["peakServicePhysicalFootprintBytes"] = 0 if USAGE_API else None
    report["phaseMemoryPeaks"] = {}
    while process.returncode is None:
        service, client = usage(process.pid), usage(os.getpid())
        if service and client:
            resident = service["rss"] + client["rss"]
            footprint = service["footprint"] + client["footprint"] if USAGE_API else None
            report["peakAggregateRssBytes"] = max(report["peakAggregateRssBytes"], resident)
            report["peakServiceRssBytes"] = max(report["peakServiceRssBytes"], service["rss"])
            if USAGE_API:
                report["peakAggregatePhysicalFootprintBytes"] = max(report["peakAggregatePhysicalFootprintBytes"], footprint)
                report["peakServicePhysicalFootprintBytes"] = max(report["peakServicePhysicalFootprintBytes"], service["footprint"])
            peaks = report["phaseMemoryPeaks"].setdefault(CURRENT_PHASE, {"rssBytes": 0, "physicalFootprintBytes": 0 if USAGE_API else None})
            peaks["rssBytes"] = max(peaks["rssBytes"], resident)
            if USAGE_API:
                peaks["physicalFootprintBytes"] = max(peaks["physicalFootprintBytes"], footprint)
            if resident > limit or (footprint is not None and footprint > limit):
                report["memoryBudgetExceeded"] = True
                process.terminate()  # Owned, unreaped subprocess only; no process-name kills.
                return
        await asyncio.sleep(.05)


def phase(name):
    global CURRENT_PHASE
    CURRENT_PHASE = name
    print(json.dumps({"event": "phase-start", "phase": name}), flush=True)


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def nodes(document):
    pending = list(document.get("children", []))
    for page in document.get("pages", []):
        pending.extend(page.get("children", []))
    while pending:
        node = pending.pop()
        yield node
        pending.extend(node.get("children", []))


def percentile(values, fraction):
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int((len(ordered) - 1) * fraction))]


async def request(port, method, route, value=None):
    body = json.dumps(value, separators=(",", ":")).encode() if value is not None else b""
    reader, writer = await asyncio.wait_for(asyncio.open_connection("127.0.0.1", port), 15)
    try:
        writer.write((f"{method} {route} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n"
                      f"Content-Type: application/json\r\nContent-Length: {len(body)}\r\n"
                      "Connection: close\r\n\r\n").encode() + body)
        await writer.drain()
        header = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 60)
        lines = header.decode().split("\r\n")
        status = int(lines[0].split()[1])
        fields = dict(line.split(":", 1) for line in lines[1:] if ":" in line)
        fields = {key.lower(): value.strip() for key, value in fields.items()}
        length = int(fields.get("content-length", "0"))
        if not 0 <= length <= 64 * 1024 * 1024:
            raise RuntimeError(f"response exceeds bounded64MiB: {length}")
        payload = await asyncio.wait_for(reader.readexactly(length), 60)
        if status != 200:
            raise RuntimeError(f"HTTP{status}: {payload[:500]!r}")
        return json.loads(payload) if payload else {}
    finally:
        writer.close()
        await writer.wait_closed()


async def tool(port, identifier, name, arguments):
    response = await request(port, "POST", "/mcp", {
        "jsonrpc": "2.0", "id": identifier, "method": "tools/call",
        "params": {"name": name, "arguments": arguments},
    })
    if response.get("id") != identifier or response.get("error"):
        raise RuntimeError(f"badRPC response: {response}")
    result = response.get("result", {})
    if result.get("isError"):
        raise RuntimeError(f"tool rejected: {result}")
    text = result.get("content", [{}])[0].get("text", "")
    return json.loads(text)


async def benchmark(args):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    document_path = output / "shared.op"
    initial = {"version": "1.0.0", "name": "Shared document benchmark", "children": [],
               "pages": [{"id": "shared-page", "name": "Shared canvas", "children": []}]}
    document_path.write_text(json.dumps(initial))
    server = args.server.resolve()
    command = [str(server), "--serve-web", "--managed", "--port", "0", "--file", str(document_path),
               "--config-root", str(output / "config")]
    child_before = resource.getrusage(resource.RUSAGE_CHILDREN)
    self_before = resource.getrusage(resource.RUSAGE_SELF)
    all_started = time.perf_counter()
    report = {"server": str(server), "serverSha256": sha(server), "command": command,
              "logicalClients": args.clients, "maxInflightRequests": min(args.inflight, args.clients),
              "roundsPerClient": args.rounds, "nodesPerRequest": args.batch,
              "expectedMutationRequests": args.clients * args.rounds,
              "expectedDrawingEdits": args.clients * args.rounds * args.batch,
              "oneSharedServiceProcess": True, "oneSharedDocumentAuthority": True,
              "externalLlmProcesses": 0, "requestRetryPolicy": "none: ambiguous writes never replayed"}
    log = (output / "server.stderr.log").open("wb")
    process = None
    sampler = None
    try:
        phase("startup")
        started = time.perf_counter()
        process = await asyncio.create_subprocess_exec(*command, stdin=asyncio.subprocess.PIPE,
                   stdout=asyncio.subprocess.PIPE, stderr=log)
        sampler = asyncio.create_task(monitor(process, report, args.max_bytes))
        handshake = json.loads(await asyncio.wait_for(process.stdout.readline(), 30))
        if not handshake.get("ok"):
            raise RuntimeError(f"server startup failed: {handshake}")
        port = handshake["port"]
        report["startupSeconds"] = time.perf_counter() - started
        phase("idle")
        idle_start = usage(process.pid)
        idle_time = time.perf_counter()
        await asyncio.sleep(args.idle_seconds)
        idle_end = usage(process.pid)
        report["idleSeconds"] = time.perf_counter() - idle_time
        report["idleServiceCpuSeconds"] = idle_end["cpuSeconds"] - idle_start["cpuSeconds"] if idle_end and idle_start else None
        await request(port, "POST", "/mcp", {"jsonrpc": "2.0", "id": "initialize", "method": "initialize",
                                              "params": {"clientInfo": {"name": "shared-document-benchmark", "version": "1"}}})
        before = await request(port, "GET", "/api/mcp/document")
        initial_nodes = list(nodes(before["document"]))
        expected = set()
        for client in range(args.clients):
            for round_index in range(args.rounds):
                for item in range(args.batch):
                    expected.add(f"agent-{client:04}-round-{round_index:02}-edit-{item:02}")
        gate = asyncio.Semaphore(args.inflight)
        start_gate = asyncio.Event()
        latencies, queue_latencies = [], []
        current = peak = accepted = 0

        async def logical_client(client):
            nonlocal current, peak, accepted
            await start_gate.wait()
            for round_index in range(args.rounds):
                queued = time.perf_counter()
                async with gate:
                    queue_latencies.append(time.perf_counter() - queued)
                    current += 1
                    peak = max(peak, current)
                    entries = []
                    for item in range(args.batch):
                        ordinal = (client * args.rounds + round_index) * args.batch + item
                        entries.append({"kind": "rect", "name": f"agent-{client:04}-round-{round_index:02}-edit-{item:02}",
                                        "x": (ordinal % 100) * 8, "y": (ordinal // 100) * 8,
                                        "width": 6, "height": 6, "fill_hex": ["#2563eb", "#059669", "#d97706", "#dc2626"][client % 4]})
                    started = time.perf_counter()
                    try:
                        result = await tool(port, f"{client}-{round_index}", "batch_design",
                                            {"nodes_json": json.dumps(entries, separators=(",", ":"))})
                        if str(result.get("wrote")).lower() != "true" or int(result.get("count", -1)) != args.batch:
                            raise RuntimeError(f"wrongeditack: {result}")
                        accepted += 1
                    finally:
                        latencies.append(time.perf_counter() - started)
                        current -= 1

        tasks = [asyncio.create_task(logical_client(client)) for client in range(args.clients)]
        await asyncio.sleep(0)
        phase("edit")
        started = time.perf_counter()
        start_gate.set()
        await asyncio.gather(*tasks)
        report["editSeconds"] = time.perf_counter() - started
        report.update(acceptedMutationRequests=accepted, peakInflightRequests=peak,
                      editsPerSecond=report["expectedDrawingEdits"] / report["editSeconds"],
                      requestsPerSecond=accepted / report["editSeconds"],
                      requestLatencyP50Seconds=percentile(latencies, .5), requestLatencyP95Seconds=percentile(latencies, .95),
                      queueLatencyP95Seconds=percentile(queue_latencies, .95))
        phase("verify")
        started = time.perf_counter()
        final = await request(port, "GET", "/api/mcp/document")
        final_nodes = list(nodes(final["document"]))
        observed = [node.get("name") for node in final_nodes if node.get("name") in expected]
        ids = [node["id"] for node in final_nodes]
        assert set(observed) == expected and len(observed) == len(expected), "lost or duplicated logical edits"
        assert len(ids) == len(set(ids)), "duplicate assigned IDs"
        assert len(final_nodes) == len(initial_nodes) + len(expected), "unexpected document nodecount"
        assert final["version"] - before["version"] == accepted, "wrong shared version increment"
        for node in final_nodes:
            if node.get("name") in expected:
                assert node["width"] == 6 and node["height"] == 6
        report.update(finalNodeCount=len(final_nodes), initialNodeCount=len(initial_nodes), uniqueAssignedNodeIds=len(set(ids)),
                      sharedVersion=final["version"], inMemoryExactEditCount=True,
                      verificationSeconds=time.perf_counter() - started)
        if not args.no_render:
            phase("render")
            started = time.perf_counter()
            rendered = await tool(port, "render", "export_item", {"itemId": "shared-page", "format": "png", "scale": "1"})
            png = base64.b64decode(rendered["bytes_base64"], validate=True)
            assert png[:8] == b"\x89PNG\r\n\x1a\n"
            report["renderDimensions"] = list(struct.unpack(">II", png[16:24]))
            (output / "shared.png").write_bytes(png)
            report.update(renderSeconds=time.perf_counter() - started, renderPngSha256=sha(output / "shared.png"))
        phase("persist")
        started = time.perf_counter()
        saved = await request(port, "POST", "/api/file/save", {"document": final["document"], "activePageIndex": 0})
        assert saved.get("ok"), saved
        persisted = json.loads(document_path.read_text())
        persisted_nodes = list(nodes(persisted))
        assert Counter(node.get("name") for node in persisted_nodes) == Counter(node.get("name") for node in final_nodes)
        assert {node["id"] for node in persisted_nodes} == set(ids)
        report.update(persistSeconds=time.perf_counter() - started, persistedExactEditCount=True,
                      persistedBytes=document_path.stat().st_size, persistedSha256=sha(document_path), correctness=True)
        phase("shutdown")
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
        log.close()
        child_after = resource.getrusage(resource.RUSAGE_CHILDREN)
        self_after = resource.getrusage(resource.RUSAGE_SELF)
        report.update(totalWallSeconds=time.perf_counter() - all_started,
                      serviceUserCpuSeconds=child_after.ru_utime - child_before.ru_utime,
                      serviceSystemCpuSeconds=child_after.ru_stime - child_before.ru_stime,
                      clientUserCpuSeconds=self_after.ru_utime - self_before.ru_utime,
                      clientSystemCpuSeconds=self_after.ru_stime - self_before.ru_stime)
        report["serviceCpuSeconds"] = report["serviceUserCpuSeconds"] + report["serviceSystemCpuSeconds"]
        report["serviceCoreEquivalent"] = report["serviceCpuSeconds"] / report["totalWallSeconds"]
        report["correctness"] = report.get("correctness", False) and report.get("serverExitCode") == 0 and not report.get("memoryBudgetExceeded", False)
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report), flush=True)


async def suite(args):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    results = []
    for count in args.clients:
        case = argparse.Namespace(**vars(args))
        case.clients, case.output = count, output / str(count)
        try:
            await asyncio.wait_for(benchmark(case), args.timeout)
        except Exception as error:
            print(json.dumps({"clients": count, "error": f"{type(error).__name__}: {error}"}), flush=True)
        path = case.output / "report.json"
        result = json.loads(path.read_text()) if path.exists() else {"logicalClients": count, "correctness": False}
        results.append(result)
        (output / "summary.json").write_text(json.dumps({"results": results}, indent=2) + "\n")
        if not result.get("correctness"):
            return False
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--binary", "--server", dest="server", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True, help="new output directory (never overwrites a document)")
    parser.add_argument("--clients", type=int, nargs="+", default=[10, 100, 1000])
    parser.add_argument("--max-inflight", dest="inflight", type=int, default=32)
    parser.add_argument("--rounds", type=int, default=2)
    parser.add_argument("--batch", type=int, default=4)
    parser.add_argument("--no-render", action="store_true")
    parser.add_argument("--max-bytes", type=int, default=1_000_000_000, help="aggregate sampled RSS/physical-footprint budget")
    parser.add_argument("--timeout", type=float, default=90, help="deadline per logical-client case")
    parser.add_argument("--idle-seconds", type=float, default=.5)
    args = parser.parse_args()
    if not (all(1 <= count <= 10000 for count in args.clients) and 1 <= args.inflight <= 48
            and 1 <= args.batch <= 64 and 1 <= args.rounds <= 100 and args.max_bytes > 0
            and args.timeout > 0 and args.idle_seconds >= 0):
        parser.error("positive bounded workloads required; max-inflight must be1..48")
    sys.exit(0 if asyncio.run(suite(args)) else 1)


if __name__ == "__main__":
    main()
