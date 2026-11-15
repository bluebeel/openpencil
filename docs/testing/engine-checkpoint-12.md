# Batched engine checkpoint 12 — 2026-11-14

Measured source: `4d51dce67d4e539d518820e5de282554a3b846db`; Jian:
`ef992bc2de292dec744eeb9181621b1c270b8e99`. The full goal remains active.
[Operation coverage](operation-coverage.md) distinguishes synthetic, selected-frame
and complete real-document evidence. This checkpoint does not establish every
operation at every agent count or a universal 1 GB memory bound.

## Implementation

Mixed drawing programs reuse three static regexes instead of compiling them on
every line. This was the dominant cost in sampled actual MCP execution. Owned
simulations and atomic batches also reuse a scoped numeric ID-floor proof;
commands that can introduce unobserved IDs invalidate it. Public mutable documents
never inherit that proof. Node JSON transformation processes each metadata object
once and retains the whole-JSON fallback for explicit child overrides.

A source-derived affine image varying adapter reproduces the Figma reference
pipeline's triangle setup, plane evaluation and edge ownership. Three frozen
photos in both sampling modes become raw RGBA exact, including a 90-degree
control that previously differed in 51 pixels / 107 components. Actual camera
construction matches 634,110 captured Float32 UV components across 24 camera
controls. Unsafe/unsupported transforms keep the previous sampler; four fixed
programs and 720 uniform bytes per draw add no image/offscreen cache.

Native text measurement restores tracking for trailing whitespace omitted by
Paragraph.visit, correcting a styled URL origin by 0.150009 px from the saved
positions. Modern decoration rectangles reuse analytic sample clipping with the
original float paint; legacy decoration remains unchanged. No font metrics,
fixture offsets or pixel-fitted curve subdivisions were introduced.

Headless raster requests and Figma conversion now share admission. Raster
capacity remains held through response writes and flushes, including disconnect
and unwind. Ordinary edits do not acquire this heavy-work gate. This prevents
independent render/import peaks, without claiming complete allocator/RSS coverage.
The HTTP writer sends headers and the borrowed body together with vectored IO,
avoiding a payload-sized copy while preserving partial-write/error behavior.

Local/managed headless modes can load explicitly selected font files before
serving requests. [Font resource configuration](headless-fonts.md) documents the
bounded path-list setting and remaining font-revision/Material Icons gaps.

## Build and focused validation

All runtime fixes were batched before the combined product build. Its first
95.50-second pass caught a font-registry dependency that was only declared for
tests. Moving it into runtime dependencies required an incremental 62.17-second
resume; both logs are retained, total 157.67 seconds. The selectively optimized
development profile matches checkpoint 11, not a full release profile.

Core/MCP focused checks pass 135 executions with one pre-existing heavy test
ignored. Native sampler and text groups pass 13 and 8. Service/export groups
pass 29 executions and font/CLI groups another 10; groups can overlap. New test
fixtures were corrected for typed state metadata and one-operation-per-DSL-line
syntax; only their five affected checks were refreshed. The dependency-scope
correction did not repeat completed tests. No whole workspace, all-pages import,
bulk-export, JavaScript or legacy browser suite was repeated.

## Actual runtime results

| Workload | Checkpoint-11 wall / CPU | Checkpoint-12 wall / CPU |
| --- | ---: | ---: |
| Three 301-command copy programs | 4.937 / 4.753 s | 0.805 / 0.633 s |
| Same programs with 24-level nested copies | 4.995 / 4.794 s | 0.806 / 0.641 s |
| Ten single-copy programs | 1.929 / 1.427 s | 1.644 / 1.179 s |
| 100 edits / 100 undo / 100 redo | 13.490 / 9.485 s | 15.086 / 9.706 s |

Large copy-program CPU falls 86.7%; nested copy CPU falls 86.6%. All acknowledged
canonical states, IDs, undo/redo and 16 PNGs match pin11 exactly. These are matched
single samples, with a measured undo wall/CPU regression retained. Across those four cases, maximum service
physical is 175.8 MB, kernel RSS 217.7 MB and aggregate RSS 274.0 MB under strict
1 GB guards. An isolated call/apply probe records a separate 92.1% CPU reduction;
its stage result is not substituted for the production measurements above.

The slow-reader control holds two 10.9 MB PNG responses without reading their
bodies. Pin11 starts a third render and attempts a conversion; pin12 refuses the
third render in 3.2 ms and conversion with HTTP 503 before decoding. Disconnecting
readers restores capacity on the first recovery probe, whose PNG is byte-exact
to pin11. Service kernel RSS is 303.3 MB versus 357.8 MB; accepted work differs by
one intentionally refused render, so this is pressure/admission evidence, not a
renderer throughput comparison. A separate 64 MiB counting-sink HTTP control
preserves wire bytes/hash while removing one approximately 64 MiB temporary copy.

Both generated 1,000-logical-client / 32-active context runs accept 2,000 requests
and 8,000 edits each. Four PNGs and name-keyed geometry/data match pin11 after the
established ID/order/empty-child normalization. Single-file edit time regresses
2.254 to 2.496 seconds / CPU 2.662 to 3.045; three-file edit time regresses 2.960
to 3.703 / CPU 6.680 to 7.953. No universal editing speedup is claimed. Kernel RSS
201.1 / 216.7 MB and physical 149.2 / 192.2 MB pass strict guards. Final persisted
content is verified; the primary live route uses explicit final save, while
alternate-file authorities save before acknowledgement.

Idle after 1,000 initialized IDs uses 0.000348 CPU seconds in 3.011 seconds
(0.01157% of one core), with document/version/durable bytes unchanged.

## Rendering parity

All six Native and six Correlated exports pass immutable resource/source/tool
identity checks with no warnings. Ten PNGs and independently decoded raw buffers
are identical to pin11. Both avatars remain raw RGBA exact / SSIM 1 in both modes.

| MUI Welcome | Native | Correlated |
| --- | ---: | ---: |
| Pin11 SSIM | 0.9996946544 | 0.9996445857 |
| Pin12 SSIM | 0.9997392305 | 0.9997528841 |
| Pin11 / pin12 RGB MAE | 0.093531 / 0.074356 | 0.082898 / 0.061392 |
| Pin11 / pin12 changed RGBA pixels | 20,613 / 19,502 | 6,622 / 5,229 |

Whole-frame RGB error falls 20.5% / 25.9%. The paragraph ROI falls 49.0% / 56.1%;
maximum byte error halves 39 to 20, with exact alpha. Larger frames and the whole
paragraph are still inexact. Selected Bridge cold exports peak at 793.2 MB
physical / 786.9 MB RSS; this is separate from complete-document operations.
Park render times regress 564 to 617 ms Native and 3,445 to 3,842 ms Correlated
in these cold samples. No rerenders were used to replace those observations.

## Complete real documents and remaining work

The new [coverage report](operation-coverage.md) records complete MUI (71 pages /
56,244 nodes) and OpenBridge (18 pages / 414,531 nodes), all four requested client
counts and missing/unsupported operation families. The live catalog has 179
tools; catalog availability does not prove benchmark coverage. Actual full-file
cases include timeouts, resource limits and correctness failures, not a blanket
pass. Client counts, admitted concurrency, accepted mutations and deferred
families remain explicit in the private reports.

OpenBridge full open requires about 2.89 GB peak RSS / 2.06 GB sampled physical
in a 5 GB read-only diagnostic, with steady state still above 1 GB. Inline nodes
alone require 1.118 GB before strings/assets. Compact optional node metadata,
streaming compatibility loading, and bounded page/asset ownership are priorities.
Full-document save comparisons also exposed unrelated 1-ULP numeric changes in
the production serde_json parser; exact parsing is an unfinished fidelity fix.
Primary atomic durable acknowledgements, selective agent undo, higher active
concurrency, remaining Boolean/font/text/image feature parity and full operation
coverage remain incomplete. The goal stays active.

Private evidence includes `optimized-engine-12/build.json`,
`program-performance-batch12-report.json`, `engine-batch-12-focused*/`,
`engine-batch-12-root-endgate/comparison.json`,
`runtime-contexts-optimized-12-agent-checks.json`,
`six-case-engine-12-comparison/summary.json`, `mui-paragraph-roi-12/report.json`,
`real-document-fixtures-pin11/summary.json`, `real-document-matrix-12/report.json`
and `json-float-roundtrip-12.json`. Proprietary design/font/image assets stay
outside the public fork.
