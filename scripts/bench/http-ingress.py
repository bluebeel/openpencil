#!/usr/bin/env python3
"""Exercise actual HTTP ingress backpressure, recovery, and idle CPU.

Two incomplete 64 MiB requests hold the 128 MiB admission budget. Other requests
must receive explicit 503 responses before their bodies or mutations are read.
The peers send only one body byte: this measures admission, not a 128 MiB import.
"""
import argparse
import asyncio
import importlib.util
import json
from pathlib import Path
import resource
import time

spec = importlib.util.spec_from_file_location("shared_benchmark", Path(__file__).with_name("headless-agents.py"))
shared = importlib.util.module_from_spec(spec)
spec.loader.exec_module(shared)


async def open_declared_request(port):
    reader, writer = await asyncio.open_connection("127.0.0.1", port)
    writer.write((f"POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n"
                  "Content-Type: application/json\r\nContent-Length: 67108864\r\n"
                  "Connection: close\r\n\r\n{").encode())
    await writer.drain()
    return reader, writer


async def response(reader):
    header = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 5)
    lines = header.decode().split("\r\n")
    fields = {key.lower(): value.strip() for key, value in
              (line.split(":", 1) for line in lines[1:] if ":" in line)}
    length = int(fields.get("content-length", "0"))
    assert 0 <= length <= 4096, length
    body = await asyncio.wait_for(reader.readexactly(length), 5)
    return int(lines[0].split()[1]), body.decode()


async def run(args):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    document = output / "ingress.op"
    document.write_text(json.dumps({"version": "1.0.0", "children": [], "pages": [
        {"id": "ingress-page", "name": "Ingress", "children": []}]}))
    command = [str(args.binary.resolve()), "--serve-web", "--managed", "--port", "0",
               "--file", str(document), "--config-root", str(output / "config")]
    report = {"command": command, "binarySha256": shared.sha(args.binary), "invariants": {},
              "heldDeclaredBodyBytes": 128 * 1024 * 1024, "heldActualBodyBytes": 2,
              "note": "Incomplete-body admission probe; not a large import memory measurement."}
    process = sampler = None
    peers = []
    stderr = (output / "server.stderr.log").open("wb")
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    started = time.perf_counter()
    try:
        process = await asyncio.create_subprocess_exec(*command, stdin=asyncio.subprocess.PIPE,
                   stdout=asyncio.subprocess.PIPE, stderr=stderr)
        sampler = asyncio.create_task(shared.monitor(process, report, args.max_bytes))
        handshake = json.loads(await asyncio.wait_for(process.stdout.readline(), 30))
        assert handshake.get("ok"), handshake
        port = handshake["port"]
        baseline = await shared.request(port, "GET", "/api/mcp/document")
        shared.phase("held-body-admission")
        for _ in range(2):
            reader, writer = await open_declared_request(port)
            peers.append(writer)
            try:
                unexpected = await asyncio.wait_for(reader.read(1), .1)
                raise AssertionError(f"admitted peer unexpectedly returned {unexpected!r}")
            except asyncio.TimeoutError:
                pass
        cpu_before = shared.usage(process.pid)
        wait_start = time.perf_counter()
        await asyncio.sleep(.2)
        cpu_after = shared.usage(process.pid)
        report["heldIdleWallSeconds"] = time.perf_counter() - wait_start
        report["heldIdleCpuSeconds"] = (cpu_after["cpuSeconds"] - cpu_before["cpuSeconds"]
                                        if cpu_after and cpu_before else None)

        async def refused():
            reader, writer = await open_declared_request(port)
            try:
                status, body = await response(reader)
                assert status == 503, (status, body)
                assert "ingress memory budget" in body, body
                return status
            finally:
                writer.close()
                await writer.wait_closed()

        refusal_start = time.perf_counter()
        statuses = await asyncio.gather(*(refused() for _ in range(args.refused_peers)))
        report["refusalWallSeconds"] = time.perf_counter() - refusal_start
        report["invariants"]["explicitRefusalsBeforeBody"] = len(statuses) == args.refused_peers
        unchanged = await shared.request(port, "GET", "/api/mcp/document")
        report["invariants"]["zeroBodyReadsRemainAvailable"] = unchanged == baseline
        for writer in peers:
            writer.close()
            await writer.wait_closed()
        peers.clear()
        # Poll only an explicitly non-mutating request after closing incomplete
        # peers. Failed writes are never replayed by this benchmark.
        shared.phase("lease-recovery")
        recovered = False
        for _ in range(100):
            reader, writer = await open_declared_request(port)
            try:
                try:
                    status, _ = await asyncio.wait_for(response(reader), .05)
                    assert status == 503, status
                except asyncio.TimeoutError:
                    recovered = True
                    break
            finally:
                writer.close()
                await writer.wait_closed()
            await asyncio.sleep(.01)
        report["invariants"]["capacityRecoveredAfterDisconnect"] = recovered
        final = await shared.request(port, "GET", "/api/mcp/document")
        report["invariants"]["noDocumentMutation"] = final == baseline
        assert all(report["invariants"].values()), report["invariants"]
        report["status"] = "completed"
    except Exception as error:
        report["status"] = "failed"
        report["error"] = repr(error)
    finally:
        for writer in peers:
            writer.close()
        if process and process.returncode is None:
            process.stdin.close()
            try:
                await asyncio.wait_for(process.wait(), 10)
            except asyncio.TimeoutError:
                process.terminate()
                await process.wait()
        if sampler:
            await sampler
        stderr.close()
        after = resource.getrusage(resource.RUSAGE_CHILDREN)
        report["serviceCpuSeconds"] = after.ru_utime + after.ru_stime - before.ru_utime - before.ru_stime
        report["wallSeconds"] = time.perf_counter() - started
        report["exitCode"] = process.returncode if process else None
        if report.get("memoryBudgetExceeded") or report["exitCode"] != 0:
            report["status"] = "failed"
        (output / "report.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2), flush=True)
    return report["status"] == "completed"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--refused-peers", type=int, default=16)
    parser.add_argument("--max-bytes", type=int, default=1_000_000_000)
    args = parser.parse_args()
    if not 1 <= args.refused_peers <= 32 or args.max_bytes <= 0:
        parser.error("refused-peers must be 1..32 and memory budget positive")
    raise SystemExit(0 if asyncio.run(run(args)) else 1)


if __name__ == "__main__":
    main()
