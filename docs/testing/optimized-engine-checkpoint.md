# Optimized engine checkpoint — 2026-11-13

Measured source: `44f1ad27fe1280d6bb27e1a16e83675b639d0d5d`, with
Jian `ede65b96c9f61776d3a138ce861134eb395153bf`. The pinned server's
SHA-256 is `48fdfb0f2dcd1ec9c940f96421352fdbecc6ae596e81cb9bb7669d755085d5e5`.

This is a development-profile build with the hot editor, MCP, importer,
renderer, layout and JSON crates at optimization level 3. Other dependencies
retain their development settings. These are not full release-profile results.
Earlier unoptimized measurements remain historical checkpoints; comparing
them also includes compiler optimization and intervening implementation changes.

## Shared document and multiple files

One headless service, two requests of four drawing edits per logical agent,
at most 32 requests in flight. Each agent opts in with its own `agentId`.
All acknowledged edits, unique IDs, expected names/geometry, saved state and
real PNG exports passed. No ambiguous write was retried.

| Logical agents | Single-file edit time | Aggregate peak physical memory |
| ---: | ---: | ---: |
| 10 | 0.0068 s | 180 MB |
| 100 | 0.0716 s | 196 MB |
| 1,000 | 2.961 s | 287 MB |

The 1,000-agent run inserted 8,000 rectangles through 2,000 MCP requests.
In-flight median/P95 request latency was 39/109 ms; P95 client-side queue
delay was 1.94 s. Whole-case service CPU was 3.71 s; service-only peak
physical footprint was 101 MB. A separate 30-second idle soak used 3.17 ms
of service CPU (about 0.011% of one core), then passed edit/export/save checks.

Across three simultaneously resident files, the same 1,000-agent workload
with `agentId` completed its edits in 9.985 s. Whole-case service CPU was
17.99 s and aggregate peak physical footprint was 266 MB. The same two
agent identities retained independent selections and cameras in every file;
these UI-only calls left all saved bytes unchanged. The legacy workload
without `agentId` also passed (8.370 s edit time, 264 MB aggregate peak).
Canonical alternate-file writes remain durable before acknowledgement.

Memory includes the load generator in addition to the shared service and
renderer. External models/agent processes are excluded. Sampling runs every
50 ms and is not an allocator hard limit. These are logical agents with a
bounded active request window, not 1,000 renderer processes.

Reproduce with [shared-file](../../scripts/bench/headless-agents.py) and
[multiple-file](../../scripts/bench/multi-file-agents.py) benchmarks using
`--clients 10 100 1000 --agent-contexts` and fresh output directories.

## Editing and rendering operations

The [operation matrix](../../scripts/bench/operation-matrix.py) passed all
13 semantic, pixel, clipboard, agent-isolation and persistence invariants
on both flat and nested 10,000-node documents with `--agent-contexts`.
Repeated operations use three serial samples; exports and clipboard actions
have one sample. These measurements are not contention tail-latency evidence.

| Operation | Flat document | Nested document |
| --- | ---: | ---: |
| Move | 5.18 ms | 3.28 ms |
| Resize | 4.12 ms | 2.59 ms |
| Duplicate | 8.67 ms | 5.97 ms |
| Text edit | 5.51 ms | 2.82 ms |
| Shadow edit | 8.72 ms | 5.13 ms |
| Component instance | 15.13 ms | 6.65 ms |
| Item PNG export | 25.63 ms | 97.09 ms |
| Selection | 1.66 ms | 1.54 ms |
| Copy | 1.44 ms | 1.20 ms |
| Paste | 32.65 ms | 11.90 ms |
| Zoom | 1.01 ms | 1.12 ms |
| Undo | 29.96 ms | 12.99 ms |
| Redo | 17.70 ms | 17.88 ms |

Whole-case service CPU was 1.22/1.35 s, with 412/647 MB aggregate physical
peaks for flat/nested documents. Export retains the containing layout root
and its required siblings, so nested item export can legitimately cost more.
The matrix's `svg-export` label means SVG import followed by PNG rendering;
vector SVG serialization is still separate, unmeasured coverage.

## Figma fidelity and remaining gaps

All six native candidate PNGs are byte-identical to the previous six-case
checkpoint after compact GUID storage. OpenBridge cold import peaked at
2.44 GB, down from 2.82 GB, and still exceeds the 1 GB target. Exact fonts,
images, authored bounds and immutable reference images remain required.

| Reference case | SSIM |
| --- | ---: |
| Park cover | 0.9959682043 |
| Material UI welcome | 0.9974204132 |
| OpenBridge cover | 0.9998078093 |
| Park avatar | 0.9999861846 |
| Material UI avatar | 0.9999810099 |
| OpenBridge header | 0.9989845348 |

These values are not exact parity. Pending query/affine/image experiments
are excluded from the pinned measurements above. Per-agent undo is also
pending: agent UI contexts are isolated, while document history stays shared.
Durable group commits, sustained mixed-operation contention, cancellation,
recovery, cold-import memory and the other operations in the
[complete goal scope](engine-goal-scope.md) still require work.

Local immutable evidence directories: `optimized-engine-1`,
`headless-contexts-optimized-1`, `headless-idle-soak-optimized-1`, `multi-file-optimized-1`,
`multi-file-contexts-optimized-1`, `operation-flat-contexts-optimized-1`,
`operation-nested-contexts-optimized-1`, and `six-case-optimized-engine-1`.
Each report records its actual executable hash and workload parameters.
Design files, fonts and oracle/runtime assets remain outside the public fork.
