# Batched engine checkpoint 6 — 2026-11-14

Measured source: `9f9908c3e24ab224528befcdb3e81df5bb6ce2f1`; Jian: `436cad53fe31178cf44d51b721c138b8999fd486`.
Six functional changes landed before one clean, unchanged-source combined build
(66.1 seconds). This is the same selectively optimized development
profile as checkpoint 5, not a full release build. The exact command, source
identity and executable records are in `optimized-engine-6/build.json`.

- Server: `de44ba5d671d3b56b6c3b6d22afbbaa1ee15f342fa3a9553ed2c2b9f18e0c1ff`.
- Exporter: `3b0a8b8ce8bf2c40fe416630beac34870fc803c26130cbf86e8f796850f0e7fb`.
- CLI: `f05869422331dc0e88ce7e6b427dd6fb60c82f2a8485b1eba21091c647b1a998`.

## Implementation

- Direct derived component caches keep validated immutable wire ranges and share
  schema/resolution tables. Reads hydrate once; unique mutation moves ownership,
  while shared mutation copies the decoded value. Color and text transformations
  retain their original order and context. Unknown fields remain available;
  generic Full decoding stays eager. Raw backing releases after hydration.
- Normalized nearest-image addressing now truncates to the measured unsigned
  16-bit texture coordinate before multiplying by the source dimension. The
  implementation handles wide dimensions without signed-integer overflow.
- Text outlines subdivide with a device-space error bound before scan conversion,
  preserving winding, closure and affine transforms. Unsupported, nonfinite or
  over-budget paths retain the existing fallback.
- Successful outlines and affine subdivisions reuse exact keys inside one export
  scope, bounded by an 8 MiB charge and 512 entries. Nested calls share the scope;
  unwinding clears it. Font, glyph, position, origin and linear-transform inputs
  determine reuse. Paragraph shaping keeps its existing behavior.
- Network hosts share a 128 MiB request-body admission budget held through dispatch.
  Admitted bodies allocate once, valid UTF-8 consumes the input allocation, and
  invalid UTF-8 expansion charges its overlapping backing before allocation.
  Refusals return 503 before mutation; error, disconnect and unwind paths release
  admission. Raw parser compatibility remains available.
- One process-wide HTTP Figma conversion runs at a time across tenants. Its lease
  lasts through response writing, preventing concurrent import graphs or slow
  readers from accumulating full converted responses. Existing identity/origin
  checks run before conversion admission.

## Validation and memory

The selected importer passed 347/347 tests; focused host transport, route,
conversion and admission filters passed 114; native text/export checks passed 19
and image checks 8. Full host/workspace suites were not run after each patch.
One six-case Native gate and one six-case Correlated gate ran at the batch end.
All input, font, reference, metadata, original-image and comparator identities
match checkpoint 5, with zero render warnings or resource failures.

All five canonical page pairs are byte-identical to the eager control. The real
MUI CLI and HTTP conversions also match checkpoint 5 exactly. Deferred decoding
has separate unknown-field, invalid-wire, ownership, transform-order, backing
release and eight-reader concurrency checks.

On matched private OpenBridge imports, graph backing before style resolution
falls from 884,835,089 to 670,675,302 bytes (214,159,787 saved). After styles it falls
from 873,395,665 to 689,441,441 bytes. Live requested heap before the histogram falls
from 1,055,143,292 to 839,514,522 bytes. Shared tables, wire/cell backing and deferred
transformation contexts are conservatively included in bounded cache charges.
A broader composite-field prototype was rejected: it retained more memory than
the selected implementation despite canonical equality.

Private matched cold physical peaks fall from 1.621 to 1.399 GB. The actual final
OpenBridge render processes peak at 1.401–1.409 GB, down from about 1.63 GB.
**Cold OpenBridge import still exceeds the 1 GB goal.** Import probes retain the
explicit 5 GB allowance; this is not a global 1 GB success claim.

## Reference parity

Raw differences below compare reconstructed straight RGBA bytes, including
transparent RGB and alpha; PNG compression and white-composited SSIM cannot hide
a mismatch. SSIM values are displayed to nine decimals; raw equality remains the
acceptance condition. Each mode has exactly two fully matching cases.

| Case | Native SSIM | Correlated SSIM | Native differing RGBA pixels | Correlated differing RGBA pixels |
| --- | ---: | ---: | ---: | ---: |
| Park cover | 0.996277786 | 0.996527026 | 86,104 | 78,618 |
| MUI welcome | 0.997480736 | 0.997442999 | 21,808 | 8,750 |
| OpenBridge cover | 0.999870968 | 0.999865187 | 5,584 | 2,506 |
| Park avatar | 1.000000000 | 1.000000000 | 0 | 0 |
| MUI avatar | 1.000000000 | 1.000000000 | 0 | 0 |
| OpenBridge update header | 0.999510683 | 0.999503457 | 1,191 | 821 |

Both avatars have SSIM 1.0 and zero raw RGB/alpha differences in both modes.
Park's former 16 differing pixels are fixed. Every larger target improves SSIM
in both modes. Text contours, underlines/checkmarks and Park soft shadows still
differ. A separate synthetic 90-degree image control remains 51 pixels / 107
absolute RGBA bytes from its reference: its remaining error is isolated to vertex
varying interpolation, while fragment-stage sampling matches exactly. No
fixture offsets, fitted matrices or interpolation workaround were added.

Contour crops with identical font bytes and saved positions improve RGB MAE
17.5–50.6%, and the Noto alpha crop improves 30.8%. Outline/subdivision reuse makes
that isolated stage 3.51–3.75 times faster over four cameras, with exact Path and
PNG equivalence. These are stage measurements, not overall renderer speedups.

There is a real cost to address next: OpenBridge cover render time increases
3.281→4.143 seconds Native and 4.008→4.839 seconds Correlated in single runs.
Its complete cold wall time increases 11.402→12.387 and 12.108→13.062 seconds.
Some decoding moves from preparation into conversion: load plus conversion changes
7.955→8.177 and 7.963→8.046 seconds, rather than doubling overall import time.
Park rendering is roughly flat; MUI welcome changes 114→128 ms Native and
600→601 ms Correlated. No statistical speedup is claimed.

## Shared-service workloads

Each context workload has 1,000 logical agents, at most 32 in-flight requests,
2,000 accepted mutation requests and 8,000 edits. External AI processes/models
are absent. Aggregate figures also include the Python load generator and enforce
both sampled RSS/physical footprint and the conservative kernel child RSS peak.
MB in the tables means 1,000,000 bytes; the budget is 1,000,000,000 bytes.

| Workload | Edit wall | Service CPU | Aggregate peak RSS | Aggregate peak physical | Kernel child peak RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| One file | 0.725 s | 1.184 s | 346.0 MB | 288.4 MB | 149.9 MB |
| Three files | 2.806 s | 6.176 s | 333.7 MB | 296.7 MB | 136.7 MB |

Both context workloads pass below 1 GB, preserve every edit and UI context, and
produce four PNGs identical to checkpoint 5. Name-keyed durable node data and
geometry match too; concurrent node IDs/order need not produce identical file
bytes. These workloads do not establish 1,000 simultaneous active renders.

Flat and nested 10,000-node operation matrices pass all 13 invariants: movement,
resize, duplicate, text, shadows, components, SVG/image pixels, clipboard, zoom,
undo/redo, context isolation and cross-file clipboard. Service CPU is 0.581/0.682 s,
aggregate physical peaks 383.6/526.7 MB, and kernel child RSS peaks 197.8/343.5 MB.
One run per workload is not statistical performance evidence. The unchanged
100,000-node layout-query suite was not repeated in this batch.

The live ingress probe holds two incomplete 64 MiB requests and obtains 16 explicit
503 refusals in 5.7 ms total. Zero-body reads remain available; disconnects restore
capacity; no document changes. It sends only two actual body bytes, so it measures
admission and recovery, not 128 MiB import throughput. Aggregate RSS peaks at
203.1 MB; idle service CPU is about 0.000032 s over 0.2 s. The same probe on checkpoint
5 times out awaiting the absent backpressure response.

Actual MUI CLI import converts all 71 pages / 56,214 nodes, resolves all 56 image
table entries and 120 typed references, adds no editor metadata, and writes exactly
the checkpoint-5 bytes. Its guarded process takes 3.165 s / 476.2 MB peak RSS.
HTTP conversion preserves authored geometry across 71 pages / 56,244 nodes; its
converted document is also byte-identical to checkpoint 5. It takes 3.194 s for
conversion, with 647.6 MB service RSS / 647.7 MB kernel child RSS. The 71,395,414-byte
response uses a private 96 MiB reader. Original document bytes/version remain
unchanged. Aggregate RSS reaches 1.039 GB under the 5 GB import allowance, so it
is not an aggregate 1 GB success.

## Evidence and remaining work

Private local reports, intentionally outside the public fork:

- `optimized-engine-6/build.json`
- `engine-batch-6-host-final/report.json`
- `lazy-derived-prototype-1/report.json` and `lazy-selected-five-pages-1/summary.json`
- `avatar-fixed16-production-photo-1/summary.json`
- `text-contour-6/summary.json`
- `six-case-engine-6-comparison/{summary,report}.json`
- `runtime-optimized-6-agent-checks.json`
- `engine-batch-6-root-endgate/report.json`
- `http-ingress-optimized-6/report.json`
- `operation-matrix-optimized-6-{flat,nested}/report.json`

The full [engine goal](engine-goal-scope.md) remains active. Next work includes
OpenBridge sampling cost, deferred hydration/preprocessing overlap, cold import
below 1 GB, remaining text/shadow and varying-interpolation parity, and broader
concurrent edit/render/persistence workloads. The six-case corpus and generated
agent workloads do not establish complete `.fig` feature parity or universal
latency/memory bounds. Proprietary designs, fonts and reference runtime artifacts
remain private.
