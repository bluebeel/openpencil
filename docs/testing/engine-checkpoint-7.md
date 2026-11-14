# Batched engine checkpoint 7 — 2026-11-14

Measured source: `284e9827abc94ac4b80848736d3d5cd87837c25a`; Jian:
`c4e32cbe43e4c4614745dbecb0692132fcdb1a85`. Independent importer, text,
image, loader and service changes landed before one unchanged-source combined
optimized build (265.55 seconds). This is a selectively optimized development
profile, not a full release build. Native libwebp is also optimized at level 3.
The exact command and source/executable identities are in
`optimized-engine-7/build.json`.

- Server: `59a2635b3406a371ee045b53198fb90659b110fb622abf471fca8c8b052f0e91`.
- Exporter: `d95fed9b423bd0270ddc4e58dabb6b95a840d863330cb9252e446c08c1a55949`.
- CLI: `88ef8d792062913b05513ed21fdf8de79a48884ef7dc2befa86fe06a60368f21`.

## Implementation

- Selected imports stream schema-validated compressed data into bounded value
  ranges, avoiding a full decompressed wire buffer overlapping the decoded
  graph. Codec validation, depth limits and the 1 GiB ceiling remain enforced;
  generic Full decoding stays eager.
- Consuming page conversion retains the complete conservative GUID dependency
  closure, including unknown containers, component swaps and overrides. It moves
  the selected page and required master subtrees instead of retaining unrelated
  source branches. All-pages and borrowed conversion retain complete semantics.
- Native text uses font underline metrics and optional skip-ink behavior, carried
  through import, schema, editor and scene data. Simple TrueType outlines preserve
  explicit font-unit points and implied midpoint rules. Unsupported CFF/composite,
  invalid or over-budget glyphs retain their existing fallback.
- A backend-owned metadata-only text LRU is bounded to 1 MiB / 2,048 entries,
  keyed by font generation, exact face and glyph. Independent live frames reuse
  metadata without rereading font table prefixes. It retains no font/table/path
  backing; existing export outline reuse remains bounded to 8 MiB / 512 entries.
- Nearest-image shaders specialize exact 16-bit addressing for source dimensions
  up to 32,768. Larger images retain the overflow-safe path. The two effects are
  cached per thread; no per-pixel branch or fixture adjustment was introduced.
- Scene loading skips presentation copies only when no bindings, scroll expansion
  or screen features require them, and releases owned prepared data before scene
  allocation. Actual presentation behavior and authored geometry are preserved.
- Bulk export and board-query snapshots capture their required results without
  deep-cloning `EditorState`. Bulk files use the existing admitted shared raster
  worker/backend. Requested IDs are resolved once with first-depth-first-match
  semantics, bounded traversal scratch and preserved partial-failure ordering.
- Desktop WebP now encodes with libwebp at quality 92 in Native and Correlated
  modes. PNG/JPEG file exports stream into a sibling temporary file before rename;
  failures preserve the old target and successful replacement preserves its
  permissions. Byte exports release the pixel surface before the final encoded
  copy. This does not add a power-loss durability guarantee.
- Bulk PNG/JPEG/JPG/WebP exports now use the requested encoder. Previously JPEG
  and WebP bulk names contained PNG bytes; individual WebP encoding also failed.
  Mobile capability gates remain unchanged.

## Focused validation

The importer passed 358/358 tests. Five complete canonical selected-page pairs
(OpenBridge 0, Park 0/14 and MUI 0/6) match the eager control, including document,
image and warning data. Unknown-field dependencies and override assignments have
dedicated closure checks. Packed-record and compressed-block-retirement prototypes
were rejected after physical-memory or semantic results failed to improve.

The final root filters passed 29 tests (10 export, 19 host). Native vector/cache
checks passed 20; additional decoration, text-raster and advance groups passed
3/5/1. UI checks passed 18, loader inheritance and schema checks one each; three
importer skip-ink checks overlap the 358 count. Image checks passed nine and loader
presentation checks 20. These groups are not a disjoint total. A targeted desktop
check passed in 57.07 seconds before the final private cache-field change; the
final native checks cover that unchanged-API cache. Three memory-scope harness
tests passed. Full workspace/host suites were not repeated after each patch.

One Native and one Correlated six-case reference suite ran at the batch end.
All 12 input/font/reference/metadata/image/comparator identities match checkpoint
6, with zero identity errors, render warnings or resource failures.

## Reference parity

Raw counts compare independently decoded straight RGBA, including transparent
RGB and alpha. SSIM is displayed to nine decimals; acceptance still requires
SSIM 1.0 and raw equality. Both avatars also retain identical candidate PNG bytes.

| Case | Native SSIM | Correlated SSIM | Native differing RGBA pixels | Correlated differing RGBA pixels |
| --- | ---: | ---: | ---: | ---: |
| Park cover | 0.996295088 | 0.996544339 | 85,841 | 78,237 |
| MUI welcome | 0.998547537 | 0.998496087 | 21,700 | 8,054 |
| OpenBridge cover | 0.999872279 | 0.999867002 | 5,542 | 2,424 |
| Park avatar | 1.000000000 | 1.000000000 | 0 | 0 |
| MUI avatar | 1.000000000 | 1.000000000 | 0 | 0 |
| OpenBridge update header | 0.999571434 | 0.999578621 | 1,154 | 704 |

All four larger targets improve SSIM in both modes. Remaining heatmaps include
text contours, links/underlines/checkmarks and Park shadows. The isolated Noto
alpha crop improves 0.161947→0.049719 with saved positions, whereas the actual full
Correlated header improves 0.161947→0.139133; the crop gain is not a full-frame
claim. The separate rotated image control still differs at 51 pixels / 107 RGBA
bytes, localized to varying interpolation. No fitted offsets were added.

OpenBridge cover rendering recovers part of checkpoint 6's regression:
4.143→3.616 seconds Native and 4.839→4.222 Correlated, about 13% in these single
samples. Preparation plus conversion also falls; conversion moves from about
1.8 to 0.5 seconds. Park preparation plus conversion rises slightly. The isolated
image-address stage improves 15–18% across three alternating rounds with identical
hashes; this is not an overall renderer speedup.

## Memory and service workloads

MB/GB here mean decimal bytes. The target is 1,000,000,000 bytes for the shared
service and renderer, excluding external models/agents. The workloads below also
report the stricter aggregate including the load generator, and check kernel
child RSS peaks. Logical agents and active request concurrency are separate.

The 101,000-node fixture exports all 1,000 frames as 320×320 PNGs. All 1,000 files
match checkpoint 6 byte-for-byte; names, formats and the original durable document
are correct. Export time falls 3.502→2.618 seconds and service CPU 3.421→2.596
seconds in matched single runs. Service RSS falls 1,169.4→854.6 MB and physical
footprint 1,023.7→709.8 MB. Aggregate RSS is 889.6 MB and kernel child RSS 854.7 MB:
the complete workload passes the strict aggregate 1 GB gate. Earlier baseline
aggregate/service failures during bulk scene derivation remain preserved.

A private 101,000-node attribution control measures a 292.9 MB live allocation
increase from the removed editor clone. Borrowing the original produces exactly
the same complete scene. Skipping a provably inactive presentation copy separately
saves about 145 MB live in the applicable presentation modes; the default bulk
mode did not use that branch.

| Workload | Logical / max active | Edit wall | Service CPU | Service RSS | Service physical | Aggregate RSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| One file | 1,000 / 32 | 0.631 s | 1.046 s | 149.4 MB | 104.5 MB | 196.3 MB |
| Three files | 1,000 / 32 | 2.625 s | 5.759 s | 136.6 MB | 112.5 MB | 185.8 MB |

Each accepts 2,000 requests and preserves 8,000 edits. Full name-keyed node data,
authored geometry and four output PNGs match checkpoint 6. Three-file UI isolation
and UI-only durable-byte invariance pass. These establish 1,000 logical contexts,
not 1,000 simultaneous renders. Service memory is essentially unchanged; the
large aggregate reduction comes from streaming executable hashes in the load
generator instead of retaining a roughly 150 MB binary buffer.

Flat/nested 10,000-node operation matrices pass all 13 invariants with service CPU
0.540/0.635 seconds and kernel child RSS 197.4/343.7 MB. Ingress admission/recovery
passes unchanged. The unchanged 100,000-node layout suite was not repeated.
Actual MCP PNG/JPEG/JPG/WebP item and bulk exports decode independently to the
correct formats, sizes, color/alpha behavior and names; document/version/durable
bytes remain unchanged. The first format harness stopped before starting a
server because Pillow was absent; an isolated harness environment resolved it.

All-pages MUI CLI import converts 71 pages / 56,214 unique nodes with 56 image
entries and 120 typed references, in 3.278 seconds / 435.8 MB physical / 484.6 MB
sampled RSS. HTTP conversion preserves authored geometry across 71 pages / 56,244
nodes in 3.307 seconds, using 3.063 service CPU seconds, 456.0 MB service physical
and 650.3 MB kernel RSS. Original durable bytes and version remain unchanged.
The HTTP aggregate reaches 1.042 GB under the explicit 5 GB import allowance.
New skip-ink fields intentionally change converted bytes: removing only these
fields, coalescing identical adjacent styles and restoring plain text when no
style remains makes the complete checkpoint-6 and checkpoint-7 documents equal.
The CLI guard did not record CPU/kernel peaks.

Cold OpenBridge physical peaks fall from approximately 1.40 to 1.202–1.218 GB in
the actual final renders. **Cold OpenBridge still exceeds the 1 GB target.** The
private requested-allocation peak around 982 MB is not physical-memory success.
An isolated 4096² noisy-PNG encoder control produces the same 57,691,976-byte PNG
in every path; streaming reduces its physical peak from 200.8 to 69.6 MB. This is
an encoder-stage result, not a universal product export bound.

## Evidence and remaining work

Private reports under the local corpus runs directory remain outside the fork:

- `optimized-engine-7/build.json`, `engine-batch-7-root-focused-final/report.json`
- `stream-five-pages-1/{implementation-report,summary}.json`
- `text-contour-7/summary.json`, `image-address-fast-batch7-1/report.json`
- `op-loader-memory-batch7-1/report.json`
- `six-case-engine-7-comparison/{summary,report}.json`
- `runtime-contexts-optimized-7-agent-checks.json`
- `runtime-imports-optimized-7-agent-checks.json`
- `bulk-exports-png-optimized-7/report.json`
- `raster-formats-optimized-7/report.json`
- `raster-encoded-buffer-overlap-7/report.json`
- `engine-batch-7-root-endgate/report.json`

The complete [engine goal](engine-goal-scope.md) remains active: cold import below
1 GB, exact larger-frame pixels, broader font/vector/effect parity, and deeper
edit/render/persistence workloads at higher active concurrency remain unfinished.
Single runs do not establish latency distributions or universal CPU/memory bounds.
Proprietary designs, fonts and reference runtime artifacts remain private.
