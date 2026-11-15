# Batched engine checkpoint 5 — 2026-11-14

Measured source: `8e57ab5432c228a78a9829a1576fab206b36dffb`; Jian: `436cad53fe31178cf44d51b721c138b8999fd486`.
One clean, unchanged-source combined build completed in 220.6 seconds.
It uses selectively optimized development crates (including the CLI), not a
full release profile. Executable hashes and exact command are in the immutable
`optimized-engine-5/build.json` report.

- Server: `fb9af6e5f72c6344a9a1b232a4ce97064e9f3556b76272fd8f7047c12d439eff`.
- Exporter: `4fa5c99cd0232f4da083105e6cb12f55ac1ba06a79b771a1682e5dfb5145fd6f`.
- CLI: `49e7e5c5891c7f0c9e159d54b3bbcc4974855cefc4d4909968d1d1396f37349c`.

## Implementation

- Import values shrink from 32 to 24 bytes; object entries 40 to 32. Shared enum
  atoms remove millions of duplicate strings; exact decompression backing and
  consuming APIs reduce source-buffer overlap. Borrowed APIs remain available.
- Service conversion borrows ordinary base64 JSON, drops an owned HTTP body
  before tree decode, and consumes compressed input. Desktop and CLI callers
  consume their input too. Route/auth/origin policy still uses the common path.
- CLI output streams to a sibling temporary file, flushes and atomically
  replaces its destination. It preserves legacy f32 widening, exact compact
  JSON/table order, images/thumbnails and failure behavior without whole-file
  JSON trees or strings. It adds no editor metadata.
- Layout queries use compact geometry and shared exact visual-bound rules.
  Borrowed schemas remove full 2,696-byte node copies from layout scratch;
  shallow ref traversal avoids cloning discarded children; exact destination
  buffers prevent Rust in-place collects retaining oversized payload buffers.
- Trusted query registration uses a shared 32 MiB / 64-entry geometry cache, weak
  owners and fresh-on-clone state identity. Keys cover document generations,
  scope, themes, fonts and relevant view modes. Unrevisioned external component
  prototypes bypass reuse. Ordinary fresh factory semantics remain available.
- Resident undo-root size measurements are weakly cached and bounded. Integral
  geometry-number byte counting avoids decimal formatting without changing
  the charge. Stale externally replaced editors drop before replacement decode.
- Exact opaque/zero paint and eligible nearest-image paths bypass expensive
  custom blending. Logical shadow width now scales correctly for 1×/2×/3× scene
  exports in Native and correlated modes; direct/live caller-scaled paths and
  arbitrary canvas callbacks keep their own scope.

## Focused validation

Importer 343/343; renderer 37; streaming schema 6 and CLI 4; cache/geometry 14;
additional shallow-reference/layout checks; host accounting/routing/import
filters; and desktop type checking passed. Filter groups overlap and are not
summed as unique tests. Complete host/workspace suites were not repeated after
small changes. There were 252 private pre/post paint/filter byte comparisons.
Native and correlated corpus checks run once per mode at this batch boundary.

## Evidence before the final runtime gate

Five matched private pages preserve canonical imported output exactly. The
retained graph+keys shrank by 228,758,652 bytes. OpenBridge matched private cold
physical peak fell from 1.949 to 1.622 GB, still above the 1 GB target. Source-release
changes reduce requested live overlap by 162 MB, but allocator retention limits
additional physical-peak savings; no proportional RSS saving is inferred.

A private canonical-model/query probe completed all 13 queries × 3 on both 100k
fixtures under a sampled 1 GB process guard (no HTTP or external generator):
718 MB flat / 730 MB nested RSS. All 14 captured old 100k hashes match; 26 outcomes
are stable; 149,985 expected rectangle geometries pass. This does not establish
HTTP/kernel peaks or repeated edit/cache-miss pressure. Those remain subject
to the actual pinned runtime gate below.

## Pinned runtime results

One service, 1,000 logical agent contexts, at most 32 active requests, 2,000
mutation acknowledgements and 8,000 drawings per workload. Authored geometry,
colors, unique IDs, saved-state checks and all four PNGs match pin 4; document
byte hashes may vary with concurrent ID/order assignment.

| Workload | Pin 4 edit time | Pin 5 edit time | Whole-case service CPU | Aggregate physical peak | Kernel child maximum RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| One file | 0.639 s | 0.705 s | 1.188 s | 287 MB | 148 MB |
| Three files | 3.812 s | 2.846 s | 6.277 s | 296 MB | 136 MB |

These are single runs. Multi-file edit time improves 25%; single-file edit time
and CPU regress slightly. Single-file whole wall time rose from 1.478 to
2.750 seconds, mostly because measured cold startup rose from 0.015 to 1.207
seconds; its cause is not isolated. No universal throughput gain is claimed.
Both workloads passed the stricter 1 GB aggregate guard and kernel RSS check.

Flat/nested 10k operation matrices passed all 13 invariants. Whole service CPU
was 0.595/0.678 seconds; aggregate physical peaks were 379/522 MB and kernel
child maxima 198/340 MB. The nested physical peak was 626 MB in pin 4. These
checks do not establish per-agent undo isolation or repeated 100k edit pressure.

CLI MUI all-pages import completed in 3.145 seconds under a 5 GB import guard,
peaking at 486 MB RSS / 451 MB physical: 71 pages, 56,214 total nodes,
56 image-table entries and 120 references. Its 25,831,742-byte output parses
and has no new editorMeta. The CLI keeps its existing OpenPencil layout mode.

The real HTTP conversion route also passed with Preserve geometry: 71 pages,
56,244 nodes, matching image counts, preserveAuthoredGeometry=true and original
server document/file/version unchanged. Conversion took 3.222 seconds; kernel
child RSS was 650 MB. Aggregate RSS reached 1.068 GB including the Python
reader, so this 5 GB-budget smoke is not an aggregate-1-GB success. Its first
attempt stopped at the helper's 64 MiB response limit; that failure is retained.
A fresh conversion-only 96 MiB reader validated the complete 71,395,414-byte
response. No ambiguous document write was replayed.

## Reference comparisons

All six Native PNG hashes exactly match Native checkpoint 3. All five other
correlated PNGs match checkpoint 4; only Park cover changes after the shadow
fix. Reference, input, fonts, image inventories and comparator identities were
verified. Every candidate rendered without resource warnings.

| Case | Native SSIM | Correlated SSIM | Correlated RGB MAE | Correlated differing pixels | Correlated render time (pin 4 → 5) |
| --- | ---: | ---: | ---: | ---: | ---: |
| Park cover | 0.9962054089 | 0.9964531066 | 0.3571219 | 80,506 | 7,700 → 2,811 ms |
| Material UI welcome | 0.9974294577 | 0.9973866602 | 0.2208276 | 10,697 | 2,257 → 600 ms |
| OpenBridge cover | 0.9998478594 | 0.9998421495 | 0.0338604 | 3,573 | 7,958 → 4,008 ms |
| Park avatar | 0.9999944670 | 0.9999944670 | 0.0039876 | 16 | 132 → 130 ms |
| Material UI avatar | 1.0 | 1.0 | 0 | 0 | 35 → 36 ms |
| OpenBridge header | 0.9993980621 | 0.9993984563 | 0.0285884 | 1,030 | 78 → 63 ms |

Park correlated SSIM rises from 0.9961374663 and RGB MAE falls from 0.3676104;
over-delta-8 pixels fall from 18,739 to 16,134. Differing pixels increase from
69,928 because faint shadow tails still differ. These metrics do not all move
in the same direction. Material UI avatar alone is exact RGBA; Park retains
16 RGB pixels and zero alpha error. Text contours, links/underlines, rotated
text/checkmarks and soft shadows remain the main residuals. Correlated mode
remains opt-in and still shapes/reconstructs text four times.

Actual native cold OpenBridge peaks are 1.629/1.631 GB for cover/header;
correlated peaks are 1.630/1.632 GB. The 1 GB cold-import target remains unmet.
The broader [goal](engine-goal-scope.md) is active; no claim of all-operation
speed, unbounded offered load or complete Figma feature/pixel parity is made.

## Actual HTTP layout memory and scope

The original stricter aggregate-1-GB gate stopped at flat 100k wide-depth-one:
1,006,469,120 bytes RSS, including the Python generator. It completed only flat
10k plus five flat 100k cases; all 18 available old hashes matched. The largest
sampled individual process was 745 MB and final 100k kernel peaks were missing,
so this failed run alone did not prove a complete service operation below 1 GB.
Its failed directory remains untouched.

A separate follow-up enforced the user's specified scope: shared service plus
renderer below 1 GB, with an independent 1.5 GB aggregate safety bound for the
external generator. The same four HTTP fixtures, 13 cases and three iterations
completed: 156 query requests, all 26 known 10k and 14 captured 100k baseline
hashes exact, all 52 saved response bodies verified, and 164,970 expected
rectangle geometries plus font/flex/reference/page/depth invariants passed.
The other 12 nested 100k before hashes remain unavailable. Flat 100k's 39
queries took 1.444 seconds /0.880 service CPU seconds; nested 1.209/0.824.
Maximum service sampled RSS was 765,739,008 bytes; physical footprint
692,372,944 bytes. Conservative kernel child maximum RSS was 765,919,232 bytes.
The generator-inclusive outer peak was 1,022,902,272 bytes RSS /937,101,496 bytes
physical. This is a service-scope success and explicitly not aggregate<1 GB.

The kernel maximum includes earlier child cases in this generator. It catches
short RSS spikes between periodic samples, conservatively bounding each case;
physical-footprint peaks remain sampled. Cached read success does not prove
repeated 100k edits/cache misses, all global resource pressure, or cold import
below the target. The service's existing upload/body and resident-file caps
also remain distinct from this workload's measured memory.

## Reproduction evidence

Immutable local directories retain commands, binaries, source/hash provenance,
limits and raw logs: `optimized-engine-5`, `engine-batch-5-host`,
`engine-batch-5-import-routing`, `engine-batch-5-desktop-check`,
`value-layout-prototype-1`, `figma-cli-stream-save-1`,
`geometry-cache-final-focused-5`, `layout-shallow-focused-5`,
`compact-query-memory-probe-5-cached`, `layout-query-optimized-5`,
`layout-query-service-scope-optimized-5`, `headless-contexts-optimized-5`,
`multi-file-contexts-optimized-5`, `operation-matrix-optimized-5-flat`,
`operation-matrix-optimized-5-nested`, `six-case-native-engine-5`,
`six-case-correlated-engine-5`, `figma-convert-mui-optimized-5-large-reader`,
and `runtime-optimized-5-agent-checks.json`.

Next priority is to avoid eager decoded instance/cache data, which accounts
for 379 MB of the remaining 885 MB owned import graph. Validated lazy wire
ranges or indexed page loading must preserve unknown fields and component
dependencies. Remaining pixel work is text/UV/shadow precision; repeated text
shaping, edit/cache-miss allocation pressure, per-agent undo and transaction /
cancellation coverage remain open in the original goal.

