#!/usr/bin/env python3
"""Probe render admission under slow readers and concurrent Figma conversion.

Two sockets stop reading multi-megabyte PNG responses. A third raster request
and a conversion must fail before allocating their work; disconnecting readers
must restore capacity. A legacy mode records the pre-fix behavior for comparison.
"""
import argparse
import asyncio
import base64
import hashlib
import http.client
import importlib.util
import json
from pathlib import Path
import random
import resource
import select
import socket
import struct
import time
import zlib

spec = importlib.util.spec_from_file_location("shared", Path(__file__).with_name("headless-agents.py"))
shared = importlib.util.module_from_spec(spec)
spec.loader.exec_module(shared)


def fixture(path):
    side = 1536
    pixels = random.Random(7851).randbytes(side * side * 3)
    scan = b"".join(b"\0" + pixels[y * side * 3:(y + 1) * side * 3] for y in range(side))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    image = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", side, side, 8, 2, 0, 0, 0))
    image += chunk(b"IDAT", zlib.compress(scan, 1)) + chunk(b"IEND", b"")
    document = {"version": "1.0.0", "editorMeta": {"preserveAuthoredGeometry": True}, "children": [
        {"type": "frame", "id": "noise", "width": side, "height": side, "children": [
            {"type": "rectangle", "id": "photo", "width": side, "height": side, "fill": [
                {"type": "image", "nearestSampling": True,
                 "url": "data:image/png;base64," + base64.b64encode(image).decode()}]}]}]}
    path.write_text(json.dumps(document, separators=(",", ":")))
    return {"width": side, "height": side, "sourcePngBytes": len(image),
            "sourcePngSha256": hashlib.sha256(image).hexdigest(), "documentSha256": shared.sha(path)}


def envelope(identifier, document):
    return {"jsonrpc": "2.0", "id": identifier, "method": "tools/call", "params": {
        "name": "export_item", "arguments": {"filePath": str(document), "itemId": "noise",
                                                "format": "png", "scale": "1"}}}


def request(port, route, body):
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=20)
    try:
        connection.request("POST", route, body, {"Content-Type": "application/json"})
        response = connection.getresponse()
        return response.status, response.read()
    finally:
        connection.close()


def raster(port, identifier, document):
    started = time.perf_counter()
    status, data = request(port, "/mcp", json.dumps(envelope(identifier, document)))
    assert status == 200, (status, data[:500])
    reply = json.loads(data)
    assert reply.get("id") == identifier
    error = reply.get("error") or reply.get("result", {}).get("isError")
    result = {"error": bool(error), "seconds": time.perf_counter() - started,
              "responseBytes": len(data)}
    if error:
        assert b"busy" in data.lower(), data[:500]
        result["reason"] = data[:800].decode()
    else:
        payload = json.loads(reply["result"]["content"][0]["text"])
        png = base64.b64decode(payload["bytes_base64"], validate=True)
        assert png.startswith(b"\x89PNG\r\n\x1a\n")
        assert struct.unpack(">II", png[16:24]) == (1536, 1536)
        result.update(pngBytes=len(png), pngSha256=hashlib.sha256(png).hexdigest())
    return result


def connections(port, document, expectation):
    blocked = []
    report = {"expectation": expectation, "blockedReaders": []}
    try:
        for index in range(2):
            shared.phase(f"blocked-reader-{index}")
            client = socket.socket()
            client.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 4096)
            client.settimeout(20)
            client.connect(("127.0.0.1", port))
            blocked.append(client)
            body = json.dumps(envelope(index, document)).encode()
            client.sendall((f"POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n"
                            f"Content-Type: application/json\r\nContent-Length: {len(body)}\r\n"
                            "Connection: close\r\n\r\n").encode() + body)
            assert select.select([client], [], [], 20)[0], "render response did not arrive"
            peek = client.recv(8192, socket.MSG_PEEK)
            header = peek.split(b"\r\n\r\n", 1)[0].decode()
            assert header.startswith("HTTP/1.1 200 OK"), header
            fields = dict(line.split(":", 1) for line in header.split("\r\n")[1:])
            length = int(fields["Content-Length"])
            assert length > 4 * 1024 * 1024, "fixture must block a normal send buffer"
            report["blockedReaders"].append({"declaredBytes": length,
                "receiveBufferBytes": client.getsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF),
                "readBodyBytes": 0})
        shared.phase("third-raster")
        report["thirdRaster"] = raster(port, 3, document)
        assert report["thirdRaster"]["error"] == (expectation == "held"), report
        shared.phase("conversion")
        status, body = request(port, "/api/figma/convert", "not json")
        report["conversion"] = {"status": status, "body": body[:1000].decode()}
        assert status == (503 if expectation == "held" else 400), report
    finally:
        for client in blocked:
            client.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
            client.close()
    shared.phase("recovery")
    report["recoveryReadAttempts"] = []
    for attempt in range(20):
        recovered = raster(port, 100 + attempt, document)
        report["recoveryReadAttempts"].append(recovered)
        if not recovered["error"]:
            report["recovered"] = recovered
            break
        time.sleep(.05)
    assert report.get("recovered"), "disconnect failed to release render capacity"
    if expectation == "legacy":
        assert report["thirdRaster"]["pngSha256"] == report["recovered"]["pngSha256"]
    return report


async def run(args):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    primary, document = output / "primary.op", output / "noise.op"
    primary.write_text('{"version":"1.0.0","children":[]}')
    shared.phase("fixture")
    fixture_info = fixture(document)
    command = [str(args.binary.resolve()), "--serve-web", "--managed", "--port", "0", "--file",
               str(primary), "--config-root", str(output / "config")]
    report = {"binarySha256": shared.sha(args.binary), "command": command, "fixture": fixture_info,
              "requestRetryPolicy": "no writes; only post-disconnect read probes may repeat",
              "maxInflightRequests": 3, "status": "running"}
    process = sampler = None
    log = (output / "server.stderr.log").open("wb")
    started = time.perf_counter()
    cpu_before = resource.getrusage(resource.RUSAGE_CHILDREN)
    try:
        shared.phase("startup")
        process = await asyncio.create_subprocess_exec(*command, stdin=asyncio.subprocess.PIPE,
                   stdout=asyncio.subprocess.PIPE, stderr=log)
        sampler = asyncio.create_task(shared.monitor(process, report, args.max_bytes))
        handshake = json.loads(await asyncio.wait_for(process.stdout.readline(), 30))
        assert handshake.get("ok"), handshake
        report["probe"] = await asyncio.to_thread(connections, handshake["port"], document, args.expect_admission)
        assert shared.sha(document) == fixture_info["documentSha256"], "read-only probe changed source"
        report["sourceUnchanged"] = True
        report["status"] = "completed"
    except BaseException as error:
        report.update(status="failed", error=f"{type(error).__name__}: {error}")
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
        report.update(wallSeconds=time.perf_counter() - started,
                      serviceCpuSeconds=cpu_after.ru_utime + cpu_after.ru_stime - cpu_before.ru_utime - cpu_before.ru_stime,
                      serverExitCode=process.returncode if process else None)
        if report.get("memoryBudgetExceeded") or report["serverExitCode"] != 0:
            report["status"] = "failed"
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2), flush=True)
    return report["status"] == "completed"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--expect-admission", choices=["held", "legacy"], default="held")
    parser.add_argument("--max-bytes", type=int, default=1_000_000_000)
    args = parser.parse_args()
    raise SystemExit(0 if asyncio.run(asyncio.wait_for(run(args), 90)) else 1)


if __name__ == "__main__":
    main()
