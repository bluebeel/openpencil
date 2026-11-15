# Batched engine checkpoint 8 — 2026-11-14

Measured source: `81d82024edda5dc0921c0625ab1dd03d9d230678`; Jian:
`fff22b0bc08babffc60a97854ffd7b4e6e37278e`. Importer, text, shadows, history,
save and MCP changes landed before one clean combined optimized build
(166.01 seconds). The selectively optimized development profile matches
checkpoint 7; this is not a full release build. Exact flags, library profiles
and executable identities are in `optimized-engine-8/build.json`.

- Server: `2e824de27f9f156a470127e4790cffd87607859d152558cb7527d63894f0bbdc`.
- Exporter: `55867c7e5cb98f8e4f4376a244cbe4f35875ccdfcde890c7b26b3d42f0b57bc7`.
- CLI: `4d352ce0d881795ad4011736e4ca72aa163aa27862f00d930ce5a11512f7bee2`.

## Implementation

- Import preprocessing uses dense GUID-indexed color metadata, shared inherited
  mode maps, typed component scopes/routes and indexed tree construction.
  Eligible geometry blobs and validated deferred wire ranges retain losslessly
  packed backing until needed. Exact byte access, clone/mutation ownership,
  concurrent reads and image-prefix detection preserve existing behavior.
- Variable-font nominal advances normalize to integer design units before the
  existing 1/1024-em grid. Shaped contextual/kerning residuals stay separate;
  direct modern paint and measurement share the same nominal rule. Legacy and
  unsupported paths keep their existing fallback and no new font cache is added.
- Imported shadow colors retain fractional RGB/alpha through the existing
  functional color representation. Functional RGB parsing divides in f64 before
  the final f32 conversion, avoiding an extra rounding step. Integer and hex
  controls retain their exact values.
- Native and CanvasKit painters map canvas-space shadow offsets back through
  the current transform, preserving the initial frame/DPR basis. Outer/inner
  shadow paint and recursive culling/layer bounds agree. Combined flip/rotation
  bounds now follow the actual painter order. Invalid/singular/perspective
  mappings fall back safely. The Gaussian blur kernel is unchanged.
- Atomic editor batches retain one undo checkpoint. Intermediate undo-only
  captures are suspended, both history stacks are parked intact, and validation,
  allocator failure or panic restores the checkpoint. Large batches no longer
  evict older entries before collapsing, and selection-only batches retain redo.
  Captures needed for actual local rollback/comparison remain complete.
- Shared canonical save snapshots now serialize the document motion preference,
  matching the owned document wire representation.
- Live-GUI MCP replaces 25 ms accept polling with blocking accept plus a bounded
  shutdown wake-up. Its lazy bounded connection workers reuse threads while
  preserving the existing 64-connection admission, 16 MiB stack allowance,
  stateful serialization and parallel stateless probes.

## Validation

Focused core filters pass 87 tests; host filters pass 52, including real TCP
ping/shutdown, admission, lean routing, worker reuse, save and agent contexts.
Importer tests pass 366/366 without warnings. Five full canonical selected-page
pairs match the eager control, including images and warnings. Text checks pass
13 native tests plus two Jian rounding/residual tests. Shadow filters pass five
export, one native transform, ten bounds, nine image, 28 mapper, three color and
11 effect checks. Mapper checks overlap the importer total; groups are not a
disjoint grand total.

CanvasKit passes nine JavaScript/math checks and one targeted wasm32 typecheck.
That first cold-target check takes 7m03s; it is separate from the one combined
optimized native build. Full workspace and unchanged codec/layout suites were
not repeated after individual patches. One Native six-case gate and one
Correlated six-case gate run after the combined build, with zero warnings,
resource failures or immutable input/font/reference/image/comparator errors.

## Reference parity

Counts compare independently decoded straight RGBA, including transparent RGB
and alpha. SSIM is displayed to nine decimals; exact acceptance still requires
SSIM 1.0 and zero raw differences. Both avatars retain identical PNG hashes in
both modes. OpenBridge cover also retains its checkpoint-7 PNG bytes.

| Case | Native SSIM | Correlated SSIM | Native differing RGBA pixels | Correlated differing RGBA pixels |
| --- | ---: | ---: | ---: | ---: |
| Park cover | 0.996670473 | 0.996904005 | 75,895 | 69,751 |
| MUI welcome | 0.998585730 | 0.998534518 | 21,309 | 7,370 |
| OpenBridge cover | 0.999872279 | 0.999867002 | 5,542 | 2,424 |
| Park avatar | 1.000000000 | 1.000000000 | 0 | 0 |
| MUI avatar | 1.000000000 | 1.000000000 | 0 | 0 |
| OpenBridge update header | 0.999863887 | 0.999871131 | 798 | 278 |

Park, MUI welcome and the header improve in both modes; Bridge cover is unchanged.
Header alpha MAE falls 0.144430→0.056378 Native and 0.139133→0.049447 Correlated.
The actual full-frame Park pixels inside the two preselected shadow regions
reduce RGB absolute error 56.1%/55.4%; this is a region measurement, not a
whole-frame percentage. Remaining differences include rotated text, checkmarks,
contours, underlines and blur. The separate image-varying and unsupported font
fallback gaps remain open.

## Performance and memory

Units are decimal MB/GB; the runtime target is 1,000,000,000 bytes for the shared
service and renderer, excluding external models/agents. Context and bulk gates
also enforce the stricter aggregate including the load generator. Logical
clients are reported separately from simultaneous accepted requests.

The matched live-MCP probe makes 128 actual serial TCP pings, without UI snapshots
or mutations. Median latency falls 35.015→0.186 ms, p95 35.153→0.273 ms, and
p99 35.254→0.333 ms. Calls take 4.233→0.026 seconds total; shutdown falls
35.28→1.70 ms. Both use the pinned optimized host libraries and the same driver.
Kernel RSS is about 10.1 MB in each. These numbers measure stateless routing,
not drawing, document snapshots, or 1,000 simultaneous operations.

A private optimized core control applies 32 nudges inside one 10,000-node frame.
The previous successful batch algorithm takes 167.8 ms versus 8.16 ms for the
new batch. Full document bytes and undo/redo results match exactly. Kernel peak
physical footprint falls 1,005.3→126.5 MB and RSS 1,007.4→129.2 MB. The new case
passes the 1 GB guard; the baseline uses the explicit 5 GB diagnostic allowance.
The 100 ms sampler misses part of these short peaks, so kernel maxima are
reported instead of claiming the smaller sampled figures. This is a core-stage
control, not an end-to-end MCP edit latency claim.

| Context workload | Logical / max active | Edit wall | Service CPU | Service RSS | Service physical |
| --- | ---: | ---: | ---: | ---: | ---: |
| One file | 1,000 / 32 | 0.635 s | 1.053 s | 151.5 MB | 106.9 MB |
| Three files | 1,000 / 32 | 2.666 s | 5.843 s | 132.3 MB | 108.2 MB |

Each accepts 2,000 requests and preserves 8,000 edits. Complete name-keyed node
data, authored geometry and four PNGs match checkpoint 7. Three-file UI isolation
and UI-only durable-byte invariance pass. Aggregate RSS peaks at 211.9/181.4 MB;
service kernel RSS is 151.7/132.4 MB. This is essentially unchanged service
performance, not evidence of 1,000 simultaneous renders.

The 101,000-node bulk fixture writes 1,000 PNGs identical to checkpoint 7, with
correct names/formats and unchanged durable input. Export takes 2.662 seconds /
2.643 service CPU seconds; service RSS is 865.6 MB, physical 720.9 MB, aggregate
RSS 900.5 MB and kernel child RSS 865.6 MB. The strict aggregate 1 GB gate passes.
Checkpoint 7 took 2.618 seconds; no new bulk speedup is claimed. Flat/nested
10,000-node operation matrices retain all 13 invariants, with service CPU
0.554/0.654 seconds and kernel child RSS 198.0/343.9 MB. Ingress recovery passes.

All-pages MUI CLI import preserves 71 pages / 56,214 unique nodes, 56 image
entries / 120 typed references, without editor metadata. It takes 3.819 seconds /
3.219 CPU seconds, with 433.2 MB sampled physical / 467.4 MB kernel RSS. HTTP
preserves 71 pages / 56,244 unique nodes and the original document/version;
conversion takes 3.571 seconds / 3.259 service CPU seconds, with 446.4 MB physical /
640.4 MB kernel RSS. Full output documents match checkpoint 7 after only the
intentional typed shadow-color changes are quantized back for comparison.
HTTP aggregate RSS reaches 1.035 GB under the explicit 5 GB import allowance.
An initial private HTTP harness selected pin7; its report is preserved and
excluded. A fresh hard-pinned pure conversion supplies the actual pin8 evidence.

Matched importer controls reduce OpenBridge requested peak 979.5→837.0 MB and
physical peak 1.201→1.150 GB, at an observed 6.1% wall-time cost. Actual final
OpenBridge renders peak at 1.156–1.164 GB physical / 1.166–1.173 GB RSS.
**Cold OpenBridge still exceeds 1 GB.** Most final preparation/render samples
are slower than checkpoint 7: Bridge cover rendering is 3.616→3.717 seconds
Native and 4.222→4.243 Correlated. Memory reduction and parity improve; a general
renderer throughput gain is not established. Single samples are not latency
distributions or causal attribution to individual patches.

## Evidence and remaining work

Private reports remain outside the public fork:

- `optimized-engine-8/build.json`
- `engine-batch-8-{core,services}-focused/report.json`
- `import-five-pages-8/implementation-report.json`
- `text-advance-8/summary.json`, `canvaskit-shadow-basis-8/summary.json`
- `shadow-precision-batch8-analysis/report.json`
- `six-case-engine-8-comparison/{summary,report}.json`
- `runtime-contexts-optimized-8-agent-checks.json`
- `runtime-imports-optimized-8-agent-checks.json`
- `bulk-exports-png-optimized-8/report.json`
- `live-mcp-8-comparison.json`, `batch-history-8-optimized-report.json`
- `engine-batch-8-root-endgate/report.json`

The full [engine goal](engine-goal-scope.md) stays active. Standalone edits inside
a large frame still expose top-level-only undo sharing: a private recursive
snapshot prototype keeps 100 edits below 140 MB kernel physical while the current
implementation crosses the 1 GB guard after 40 captures. This prototype is not
integrated; schema/wire, reader, repair and resident-accounting APIs still need
production changes. Cold import, exact larger-frame pixels, broader feature
parity, higher active concurrency, selective undo and persistence/recovery
coverage remain unfinished. Proprietary designs, fonts and runtime artifacts
remain private.
