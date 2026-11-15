# Batched engine checkpoint 10 — 2026-11-14

Measured source: `944b22cc45730a70383055872aaec5347690881d`; Jian:
`ef992bc2de292dec744eeb9181621b1c270b8e99`. All fixes landed before one clean
combined optimized build, taking 114.22 seconds. The selectively optimized
development profile matches checkpoint 9; this is not a full release build.
Exact flags, profiles and library identities are in `optimized-engine-10/build.json`.

- Server: `1e3b01b70bda14e0d9dcd815f5cd7fa4856b4f2ac379a892afb6195ea5276cd2`.
- Exporter: `9939e45562bff4ca0b905cabb9be08acec65247af4703cefe21729009a8d9598`.
- CLI: `7b1eb5c6b4a564c61860b78f8f9cb07834013512e27630be9bb9473bbdc85965`.

## Changes

Direct atomic batches reuse the exact checkpoint just captured by their outer
undo boundary. Nested operations and maintenance callbacks keep conservative
fresh rollback captures, including unversioned writes to the public document.
The disposable simulation clone preserves all editor fields, validation,
allocation and revision behavior, while omitting retained undo history and
undo-only captures. Atomic rollback checkpoints remain active. An experimental
streaming sensitivity comparison had no material gain and was reverted; exact
checkpoint-9 numeric/map-order/image-scope checks remain unchanged.

The host ledger skips already-accounted immutable edge arrays and caches a
bounded, weakly held union of retained history allocations. Cache storage is
itself charged, capped at 32 MiB / 65,536 entries, and discarded under tight
limits in favor of complete accounting. A paired optimized probe across 200
real inserts reduced accounting from 1.8170 to 0.2586 seconds. Every logical
retained charge matched after separately accounting for the 16.03 MB cache.
This stage gain is distinct from the end-to-end results below.

Mixed MCP drawing programs now compute their outcome against a borrowed source
editor, retaining one independent simulator instead of an extra complete editor
copy. The one-request registry moves the owned command after the borrow ends.
The common `nodes_json` path and reusable public snapshot APIs retain their
existing adapters. Full and Lean catalogs preserve wire results, atomic rejection
and source-state isolation.

Two functional MCP gaps are fixed. The 37 advertised UIKit tools use `insert_*`
names, but dispatch previously loaded them only for `add_*` requests. Targeted
lookup now resolves the actual names without constructing every tool, preserves
last-registration-wins name collisions, and rejects nonfinite coordinates.
Every advertised built-in tool passes actual dispatch plus exact undo/redo.
Copy operations also stop registering live source IDs as aliases to their copies;
repeated copy/delete and later updates to the original node/children now keep
correct targets. A real checkpoint-9 benchmark exposes the original failure.

Rounded-shadow painting partitions a conservatively constant interior from the
analytic perimeter. The framebuffer blender and shader arithmetic are unchanged;
an inverse-transform/device-pixel guard shrinks or disables the fast path when
needed. Two fixed effects per render thread replace per-pixel work without new
offscreen buffers. The separate legacy Jian browser bridge now carries float
RGBA and the blur profile, with bounded fixed-effect reuse and canvas-space
offsets. Its JavaScript and wasm checks pass; browser visual equivalence is not
established by the Native corpus.

## Validation and exact output

Final focused groups pass 108 core, 89 MCP, 57 service/ledger/authority and 21
renderer test executions; groups overlap. The component-dispatch test covers
all 37 advertised tools. Nineteen Node JavaScript checks and the targeted legacy
web wasm check pass. Two new fixture failures were corrected to use one operation
per DSL line and a genuinely rejecting update; unknown deletes intentionally
remain no-ops. Only affected filters were repeated, not full workspace suites.

One Native and one Correlated six-case gate verify all immutable design, font,
image, reference, metadata, comparator, runner and guard identities. All 12 PNG
hashes and independently decoded straight-RGBA buffers equal checkpoint 9
exactly, with no warnings. Both avatars remain exact Figma matches; larger-frame
SSIM/raw differences remain as recorded in [checkpoint 9](engine-checkpoint-9.md).

Park full-frame render time falls 613.05 to 567.60 ms Native (7.4%) and 4.169 to
3.458 seconds Correlated (17.1%). All four OpenBridge cold processes remain below
1 GB: maximum physical footprint 795.2 MB and RSS 804.7 MB. These are single
matched samples. Correlated Bridge-cover rendering rises 4.232 to 4.338 seconds
(2.5%); Correlated Park-avatar physical footprint rises about 340 to 419 MB with
similar 501 MB RSS. A separate wide-shadow control was slower too. No universal
renderer speedup or memory reduction is inferred.

## Shared-service workloads

Units are decimal MB/GB. The target is 1,000,000,000 bytes for the shared service
and renderer, excluding external models/agents. Runtime workloads also enforce
the stricter aggregate including the load generator, with sampled RSS/physical
footprint and kernel child RSS high-water. These are measured bounds, not an
allocator hard limit for arbitrary document sizes or offered load.

| Context workload | Logical / max active | Edit wall | Service CPU | Service physical | Aggregate physical |
| --- | ---: | ---: | ---: | ---: | ---: |
| One file | 1,000 / 32 | 2.702 s | 3.142 s | 150.6 MB | 184.9 MB |
| Three files | 1,000 / 32 | 3.051 s | 6.934 s | 189.4 MB | 224.4 MB |

Both preserve 2,000 accepted requests / 8,000 edits, unique IDs, full name-keyed
fields/geometry/colors, four PNG byte sequences and six multi-file UI contexts.
Generated IDs/order and absent/empty children use the established comparison
normalization; UI-only actions leave durable bytes unchanged. Single-file time
is essentially unchanged from 2.690 seconds. Three-file edit time falls 22.8%
and CPU 21.2%, while service physical memory rises from 164.1 to 189.4 MB. Strict
1 GB guards pass. These runs have 32 active requests, not 1,000 simultaneous renders.

A separate fresh service accepts 1,000 distinct agent IDs through UI-only reads
at 32 maximum active requests. During 3.002 seconds with no HTTP requests,
service CPU advances 0.000501 seconds, or 0.0167% of one core. Sampled service
physical memory is 11.68 MB / RSS 25.44 MB; document/version/durable hash remain
unchanged. HTTP does not expose a resident-context count, so successful distinct
agent initialization is the directly verified fact.

The 10,000-node standalone history workload again verifies 100 edits, 100 undos,
100 redos, every durable state including child presence, and four restored PNGs.
Wall time falls 16.974 to 13.730 seconds; service CPU 12.775 to 9.735 seconds
(23.8%). Edit p50/p95 is 29.14/31.81 ms, undo 33.55/34.42 ms, redo 33.51/34.37 ms.
Service physical memory rises 148.9 to 163.3 MB, kernel RSS is 202.9 MB and
aggregate RSS 259.7 MB. The bounded cache trades additional memory for CPU.

The new drawing-program benchmark copies/moves/deletes a 33-node subtree, then
makes a retained leaf edit in a 10,000-leaf document. Ten single-copy programs
plus undo/redo and PNG checks take 1.906 seconds versus 2.122 on pin9. Program
p50 falls 67.74 to 54.26 ms; program CPU falls 0.689 to 0.551 seconds (20.0%).
Every acknowledged document and restored PNG matches. Service physical is
141.9 MB / kernel RSS 184.1 MB. The larger case runs three 301-command programs
(100 copies each) in 5.514 seconds / 5.307 service CPU seconds, with 161.3 MB
physical / 203.5 MB kernel RSS. Pin9 fails this case because deleted-copy aliases
break subsequent source lookups, so its preserved failure is excluded from
performance comparison. Pin10 passes the full document/undo/redo/PNG contract.

Bulk export writes 1,000 PNGs from 101,000 nodes, byte-identical to pin9, in
2.654 seconds. Service physical is 714.8 MB, kernel RSS 860.9 MB and aggregate
RSS 895.7 MB. Flat/nested 10,000-node operation matrices retain all 13 invariants,
with 0.636/0.719 service CPU seconds and 200.7/265.0 MB kernel RSS.

Production CLI and HTTP all-pages MUI conversions preserve all 71 pages, image
references, counts and exact complete canonical bytes from pin9; HTTP authority,
version and source input stay unchanged. Both children verify supplied pin10
source/Jian/path/hash before process creation. CLI takes 3.739 seconds / 3.300
CPU seconds with 456.5 MB physical / 522.4 MB kernel RSS. HTTP conversion takes
3.980 seconds / 3.569 service CPU seconds with 459.8 MB physical / 694.8 MB
kernel RSS, under the explicit 5 GB import allowance. No throughput improvement
is claimed for unchanged import conversion.

## Evidence and remaining work

Private reports stay outside the public fork:

- `optimized-engine-10/build.json`
- `core-history-batch10-final-focused/report.json`
- `engine-batch-10-final-dispatch-focused/report.json`
- `engine-batch-10-corrected-fixtures-focused/report.json`
- `engine-batch-10-host-fixture-final/report.json`
- `resident-charge-batch10-report.json`
- `shadow-render-batch10-focused/report.json`
- `shadow-throughput-batch10-5/report.json`
- `six-case-engine-10-comparison/{report,summary}.json`
- `engine-batch-10-root-endgate/report.json`
- `runtime-contexts-optimized-10-agent-checks.json`
- `idle-contexts-optimized-10/report.json`
- `runtime-imports-optimized-10-agent-checks.json`

The full [engine goal](engine-goal-scope.md) remains active. Common single-file
undo capture still validates the document in O(N); larger drawing programs still
repeat global ID collection and subtree JSON conversion. Faster exact handling
of those paths, remaining larger-frame text/contour/underline/dither parity,
selective per-agent undo, higher active concurrency, shared memory admission,
and broader cancellation/idempotency/durability coverage remain unfinished.
