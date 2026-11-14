# Batched engine checkpoint 9 — 2026-11-14

Measured source: `dbd2faa8a7ebda80d4b1460c3db5425cf10b61ac`; Jian:
`6ebdba46358ff410e4db22182edd0f6a717306ec`. All implementation changes landed
before one clean combined optimized build, taking 238.74 seconds. The profile
matches checkpoint 8: selected engine/renderer/importer/MCP/CLI dependencies
are optimized in the development profile; this is not a full release build.
Exact flags, library profiles and identities are in `optimized-engine-9/build.json`.

- Server: `d616495313af8ce47d81117dc09b8e54122bc5161b9b81b39c1d7ff13ee3ade5`.
- Exporter: `980d0de979738b3e48e2afdf794f576f6d31fd7b7d1c15c25342dc795dd55550`.
- CLI: `02ed692b1f06ce506cb97bdcefe1d78d383ac06678e3074c3234449627ef5a5f`.

## Implementation and checks

Undo snapshots now share unchanged descendants, document metadata, components
and root/page/container edge arrays recursively. Readers explicitly borrow node
metadata or materialize an owned subtree; canonical serialization preserves
field order, child presence, numeric bits, JSON map order and image-save scope.
Copy-on-write repair detaches affected paths. One 10,000-root pointer array
occupies 80,040 bytes across 100 cloned checkpoints, replacing 8 MB of duplicate
root-pointer copies. Resident accounting deduplicates all shared allocations and
keeps bounded weak caches. Searching mutable nodes no longer changes untouched
rectangle children from absent to empty.

Primary-file, resident-file, live and in-memory MCP command dispatch now gives
standalone document writes one undo boundary, including synchronous repairs.
Atomic batches retain one entry. Failed primary saves restore content, history,
selection and a replaced cut clipboard through a shared checkpoint. Tests cover
failed undo and redo saves too; the revision allocator remains monotonic.
Undo is still document-wide, not selective per-agent undo.

Owned `.fig` preparation validates and indexes exact wire frames, keeps compact
page/topology metadata, then hydrates only selected render dependencies and
required global definitions. Selected imports keep raw blobs to reduce CPU;
borrowed/all-pages paths retain complete eager semantics and packing. Duplicate
IDs or alternate root layouts take the complete fallback. Public APIs stay
unchanged, and deferred work is attributed to the phase that actually runs it.

Imported uniform, unsmoothed rounded rectangles carry an optional triangular
shadow profile through scene/native/Jian and production CanvasKit painters.
Missing profiles retain Gaussian defaults. The shared rounded geometry handles
signed spread and stroke inset, and the shader uses four midpoint samples of
an analytic triangular CDF. Functional RGBA survives legacy Native shadow
layers. The Web runtime-effect cache evicts and disposes entries at 128 entries
or 2 MiB of charged UTF-16 source; compiled GPU memory is outside that byte count.

Final focused groups pass 101 core filter executions, 370 importer tests plus
one encoded fallback test, 54 renderer/UI/loader/native checks, and 41 service
checks. These groups overlap. Fifteen Node JavaScript checks and one targeted
CanvasKit wasm32 typecheck pass. A native test adapter needed one incremental
compile retry; one harmless test-only unused import warning remains. Full
workspace suites were not rerun after each change.

## Reference parity

One final Native and one Correlated six-case gate use the pinned exporter.
Independent straight-RGBA decoding verifies transparent RGB and alpha, and all
input, font, reference, image and comparator identities match the prior gate.
There are no reference warnings or resource failures. Exact acceptance requires
SSIM 1.0 and zero raw differences, rather than a rounded displayed score.

| Case | Native SSIM | Correlated SSIM | Native differing RGBA pixels | Correlated differing RGBA pixels |
| --- | ---: | ---: | ---: | ---: |
| Park cover | 0.996723068 | 0.996969131 | 55,575 | 39,208 |
| MUI welcome | 0.998585730 | 0.998534518 | 21,309 | 7,370 |
| OpenBridge cover | 0.999872279 | 0.999867002 | 5,542 | 2,424 |
| Park avatar | 1.000000000 | 1.000000000 | 0 | 0 |
| MUI avatar | 1.000000000 | 1.000000000 | 0 | 0 |
| OpenBridge update header | 0.999863887 | 0.999871131 | 798 | 278 |

Both avatars remain exact, and all ten non-Park-cover PNGs retain checkpoint-8
bytes. Park cover improves from 0.996670473/0.996904005. On the two previously
fixed shadow regions, actual full-frame RGB error falls 46.8% Native and 63.2%
Correlated. These are region results, not whole-frame percentages. No framebuffer
dither phase or fixture-specific correction was promoted.

## Memory and performance

Units are decimal MB/GB. The target is 1,000,000,000 bytes for the shared service
and renderer, excluding external models/agents. Runtime context, undo, bulk and
operation gates also enforce the stricter aggregate including the load generator.
Kernel RSS high-water complements sampled RSS/physical footprint. Passing these
workloads is not an allocator hard limit for arbitrary documents.

All four OpenBridge cold reference exports now stay below 1 GB: maximum physical
footprint 798.4 MB and RSS 807.9 MB, versus approximately 1.16 GB physical in
checkpoint 8. Native Bridge-cover process time falls 11.689 to 9.196 seconds;
preparation falls 7.314 to 4.350 seconds, while conversion rises 0.534 to 1.137
seconds as deferred work moves there. Native render time is 3.590 seconds.
The final paired importer controls preserve five full canonical page outputs,
components, images and warnings exactly. A separate strict aggregate 1 GB
import proof also passes with its recorded mapper/source hashes.

The actual MCP undo benchmark executes 100 standalone leaf edits, 100 undos,
100 redos and four PNG exports in one 10,000-node frame. Every acknowledged
durable document matches expected data, including unrelated absent child fields;
undo restores the original PNG and redo restores the edited PNG byte for byte.
It takes 16.974 seconds including client validation, with 12.775 service CPU
seconds, 148.9 MB service physical, 188.4 MB kernel RSS and 244.9 MB aggregate
RSS. Edit p50/p95 is 35.80/40.91 ms; undo 45.71/47.70 ms; redo 45.55/47.85 ms.
The preserved checkpoint-8 negative control changes unrelated child fields on
its first edit and fails, so it is not a valid timing baseline.

| Context workload | Logical / max active | Edit wall | Service CPU | Service physical | Aggregate physical |
| --- | ---: | ---: | ---: | ---: | ---: |
| One file | 1,000 / 32 | 2.690 s | 3.161 s | 148.7 MB | 182.7 MB |
| Three files | 1,000 / 32 | 3.954 s | 8.795 s | 164.1 MB | 199.2 MB |

Each accepts 2,000 requests and preserves 8,000 edits. All four PNGs match
checkpoint 8; name-keyed fields/geometry match after excluding generated IDs
and normalizing the intentional absent/empty-child fix. Six agent/document UI
contexts and UI-only durable-byte invariance pass. Kernel RSS is 200.5/188.8 MB.
**Editing regresses** from 0.635/2.666 seconds and 1.053/5.843 service CPU seconds
in checkpoint 8. The new undo/snapshot path needs CPU optimization; these results
do not establish 1,000 simultaneous renders or uniformly fast operations.

Bulk export writes 1,000 PNGs from 101,000 nodes, all byte-identical to checkpoint
8, in 2.651 seconds / 2.627 export CPU seconds. Service physical is 721.0 MB,
kernel RSS 865.9 MB and aggregate RSS 900.7 MB; the strict 1 GB gate passes.
Both 10,000-node flat/nested operation matrices retain all 13 invariants.
Service CPU is 0.632/0.726 seconds and kernel RSS 200.9/265.0 MB.

MUI all-pages CLI and HTTP outputs match the final eager loader byte for byte.
Both preserve 71 pages and the established node/image/reference counts; HTTP
leaves its original document/version untouched. Relative to checkpoint 8, each
output has exactly 1,955 added triangular-kernel fields and no other changes.
CLI takes 3.740 seconds / 3.349 CPU seconds with 519.0 MB kernel RSS. HTTP
conversion takes 3.955 seconds / 3.607 service CPU seconds with 476.6 MB service
physical and 697.5 MB kernel RSS. HTTP aggregate RSS is 1.163 GB under the explicit
5 GB import allowance; the service itself is below 1 GB. An initial private CLI
harness selected pin8; its failure is preserved and excluded. Corrected child
runners verify pin9 source, path and binary hash before starting either process.

Park's more accurate shadow shader costs CPU: Native rendering rises from
352.3 to 613.0 ms and Correlated from 2.786 to 4.169 seconds. Faster loading
reduces the Native total from 2.034 to 1.853 seconds, but it does not remove the
shader regression. These are single matched runs, not latency distributions.

## Evidence and remaining work

Private evidence stays outside the public fork:

- `optimized-engine-9/build.json`
- `recursive-history-production-9/report.json`
- `engine-batch-9-services-final-focused/report.json`
- `shadow-render-batch9-focused/report.json`
- `import-final-current-five-pairs-9/summary.json`
- `import-final-all-pages-mui-9/summary.json`
- `six-case-engine-9-comparison/{report,summary}.json`
- `engine-batch-9-root-endgate/report.json`
- `undo-history-optimized-9/report.json`
- `runtime-contexts-optimized-9-agent-checks.json`
- `runtime-imports-optimized-9-correct-pin-agent-checks.json`

The full [engine goal](engine-goal-scope.md) remains active. Next priorities are
undo/capture CPU and triangular-shadow rendering cost, followed by remaining
larger-frame text/contour/underline/dither differences. Live documents and undo
restores still materialize owned node trees. The separate legacy Jian browser
DrawOp bridge still drops the new shadow profile; arbitrary masks/nonuniform
rounded triangular layers and legacy Gaussian sigma conventions remain gaps.
Higher active concurrency, selective per-agent undo, broader feature parity,
and cancellation/idempotency/durability coverage remain unfinished. Proprietary
designs, fonts, captures and runtime outputs are not published.
