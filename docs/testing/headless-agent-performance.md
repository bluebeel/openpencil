# Shared headless agent performance

Run many logical clients against one document authority and one headless
service. External model/agent processes are excluded; the load generator is
included in the aggregate memory measurement. Each client sends sequential
MCP batches, and a bounded request window provides backpressure.

```sh
cargo build -j1 -p op-host-web-server
python3 scripts/bench/headless-agents.py \
  --binary target/debug/op-host-web-server \
  --clients 10 100 1000 --max-inflight 32 \
  --max-bytes 1000000000 --output /tmp/openpencil-agent-benchmark
```

Use a new output directory. The benchmark creates its own synthetic document;
it never opens or overwrites a user document. A case succeeds only when every
accepted write, unique assigned node ID, expected node name and geometry is
present in memory and in the saved file, the version advances exactly once
per mutation request, and a real raster export succeeds. Ambiguous writes
are never replayed. Reports retain binary hashes, request/queue latency, CPU,
RSS, macOS physical footprint, phase peaks and PNG/persisted-document hashes.
The memory sampler runs every 50 ms; this is a sampled budget, not an allocator
hard limit. Linux uses RSS when physical-footprint accounting is unavailable.

## Measured checkpoint: 2026-11-13

Development binary, macOS ARM64. Each case starts with an empty document and
uses two rounds of four inserted rectangles per logical client. These results
cover insertion, export and save, not arbitrary editor operations or imported
Figma complexity.

| Logical clients | Max requests in flight | Drawing edits | Edit time | Aggregate peak physical memory |
| ---: | ---: | ---: | ---: | ---: |
| 10 | 10 | 80 | 0.011 s | 201 MB |
| 100 | 32 | 800 | 0.168 s | 207 MB |
| 1,000 | 32 | 8,000 | 13.576 s | 302 MB |

All cases passed exact in-memory and persisted-state checks. The 1,000-client
case used 14.54 seconds of total service CPU (about one core during its
14.97-second total run); its service-only peak physical memory was 99 MB.
Its 95th-percentile in-flight request latency was 454 ms, with 9.01 seconds
of 95th-percentile client queue delay. Its 798 × 638 PNG took 184 ms. This
is 1,000 logical clients with 32 active requests, not 1,000 simultaneous
renderer processes or a claim of interactive latency at unlimited offered load.

The same 8,000-edit workload previously exceeded its 90-second deadline.
Profiling identified a repair pass that searched the entire forest for every
top-level node after every edit. It now collects image-slot repairs in one
forest traversal, retaining the existing validated command application.
Request-scoped batch snapshots also avoid full editor copies for simple
writes; JavaScript expansion runs once before selecting the required state.

## Memory and parity boundaries

Native SVG raster reuse is bounded to 32 MiB and 256 entries. Compiled shader
reuse is bounded to 128 entries and 2 MiB of retained source; compiled Skia
heap allocations are not included in that source-byte ledger. These limits
evict acceleration caches without changing raster resolution or document data.

The shared-service target remains below 1 GB, with low idle CPU and bounded
render concurrency. Large `.fig` import is not there yet: OpenBridge cold
import still peaks around 4.1 GB. Move, resize, duplicate, text, effects,
components, SVG, images, copy/paste, viewport changes and multiple simultaneously
open documents need their own operation and pixel benchmarks. The six frozen
Figma comparisons remain the independent fidelity gate; speed does not imply
pixel parity.
