# Shared headless agent performance

Run many logical clients against one document authority and one headless
service. External model/agent processes are excluded; the load generator is
included in the aggregate memory measurement. Each client sends sequential
MCP batches, and a bounded request window provides backpressure.

Coverage is [explicitly incomplete](operation-coverage.md): earlier context gates
use generated documents; new full real-file cases add partial results and failures.
Current multi-client cases use at most 32 active requests. Selected-frame memory
results do not establish a full OpenBridge/MUI memory bound.

The latest [batched checkpoint](engine-checkpoint-13.md) reduces full MUI service
RSS to 382 MB, preserves exact f64 values and saves Managed primary commands
before acknowledgement. It records the first-edit OpenBridge memory failure,
a 10-client MUI empty-child correctness failure, mixed CPU results and unchanged
reference PNGs. Higher client counts remain budget-limited rather than all-operation
passes.
The [previous batched checkpoint](engine-checkpoint-12.md) cuts the measured large
copy-program CPU by 86.7%, improves text/image sampling fidelity, adds shared
heavy-work admission and configurable headless fonts. It also records slower
context/undo/Park samples and full-file resource/precision failures. The
[real-document harness](../../scripts/bench/real-document-operations.py) explicitly
records incomplete 1/10/100/1,000-client coverage and diagnostic allowances.
The [previous batched checkpoint](engine-checkpoint-11.md) combines copy and
component-inheritance fixes, cheaper exact history capture, ID validation and
ligature/spacing parity before one build. MUI Native SSIM rises to 0.999695;
copy-program CPU falls about 10% and single-file edit CPU about 15% in matched
runs. All bounded context and cold-export memory gates pass below 1 GB.
The [previous batched checkpoint](engine-checkpoint-10.md) reduces multi-file,
undo, program and shadow CPU costs while retaining all 12 checkpoint-9 PNGs.
It fixes advertised kit-tool dispatch and repeated-copy references, and records
strict 1 GB workload gates plus a 1,000-agent-ID idle sample. Single-file
insertion remains a CPU optimization target.
The [previous checkpoint](engine-checkpoint-9.md) integrates recursive undo
sharing, failed-save recovery, indexed imports and rounded-shadow profiles.
Cold OpenBridge exports now fit below 1 GB in the measured suite; standalone
undo, contexts, bulk exports and operation gates pass. Undo and shadow CPU
regressions are measured and remain optimization targets.
The [previous checkpoint](engine-checkpoint-8.md) improves text/shadow parity,
reduces importer memory, fixes atomic-batch undo and motion saves, and removes
live-MCP accept polling. It records matched live-ping and undo-stage gains,
unchanged agent/bulk invariants, and remaining cold-import and exact-pixel gaps.
The [previous checkpoint](engine-checkpoint-7.md) adds streamed imports,
improved text/underline parity, faster exact image addressing and shared bulk
exports below 1 GB on the 101,000-node workload. It also fixes WebP and bulk
format encoding and records the remaining cold-import and exact-pixel gaps.
The [previous checkpoint](engine-checkpoint-6.md) adds lazy component-cache
decoding, exact avatar sampling, scoped outline reuse and bounded HTTP admission.
The [previous checkpoint](engine-checkpoint-5.md) includes borrowed layout inputs,
bounded shared geometry reuse, compact imports and streaming CLI saves.
The [previous checkpoint](engine-checkpoint-4.md) records ID allocation and
authored-content preservation improvements.
The [previous checkpoint](engine-checkpoint-3.md) covers bounded worker reuse
and the earlier rendering and query improvements.
The [earlier optimized checkpoint](optimized-engine-checkpoint.md) also includes
flat/nested operation matrices.
The development measurements below are earlier historical checkpoints.

```sh
cargo build -j1 -p op-host-web-server
python3 scripts/bench/headless-agents.py \
  --binary target/debug/op-host-web-server \
  --clients 10 100 1000 --max-inflight 32 \
  --max-bytes 1000000000 --output /tmp/openpencil-agent-benchmark
```

The standalone history gate exercises 100 individual edits, 100 undos, 100 redos
and four PNG exports inside one 10,000-node frame. It checks every acknowledged
saved document, including unrelated child-field presence, and exact restored PNGs.

```sh
python3 scripts/bench/undo-history.py \
  --binary target/debug/op-host-web-server \
  --leaves 10000 --edits 100 --max-bytes 1000000000 \
  --output /tmp/openpencil-undo-benchmark
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
import now peaks around 2.44 GB (previous checkpoints: 2.82 GB and about 4.1 GB). All six candidate
PNGs are byte-identical after compact GUID storage. The optimized checkpoint
now includes separate operation and multiple-file benchmarks. The six frozen
Figma comparisons remain the independent fidelity gate; speed does not imply
pixel parity.

## Borrowed identity queries

The allocator high-water query now visits borrowed ID strings instead of
allocating a second document-wide collision set. The real collision set remains
complete and reserves the known root count. The same 10,000-node operation
matrix passed all semantic, pixel and persistence checks with the following
serial median latencies (three samples per repeated operation):

| Operation | Previous | Borrowed ID query |
| --- | ---: | ---: |
| Move | 22.8 ms | 10.1 ms |
| Resize | 21.6 ms | 7.4 ms |
| Text | 21.8 ms | 5.6 ms |
| Shadow | 18.6 ms | 6.9 ms |
| Duplicate | 27.9 ms | 9.7 ms |
| Component instance | 34.7 ms | 16.1 ms |
| Zoom | 17.2 ms | 4.9 ms |

The complete case used 3.98 seconds of service CPU versus 5.40 seconds before,
with 432 MB aggregate peak physical footprint. The 1,000-client insertion case
used 14.01 seconds of service CPU versus 14.54 seconds before; its wall time
was similar, so these changes are not presented as a major insertion-throughput
improvement.

A deterministic two-client case reproduces the legacy global UI-context
behavior when `agentId` is absent: interleaved selection/clipboard and viewport
calls share state. Opt-in [agent contexts](agent-contexts.md) isolate these
values; the latest operation matrices verify that behavior and cross-file paste.
Undo history remains shared.

Run the operation matrix separately from contention tests:

```sh
python3 scripts/bench/operation-matrix.py \
  --binary target/debug/op-host-web-server --nodes 10000 --iterations 3 \
  --output /tmp/openpencil-operation-matrix
```

This serial matrix covers production move, resize, duplicate, text, shadow,
component creation/instancing, SVG import, PNG image export, selection,
copy/paste, viewport changes and undo/redo. Its `svg-export` metric means an
imported SVG rendered to PNG; vector SVG serialization is separate coverage.
