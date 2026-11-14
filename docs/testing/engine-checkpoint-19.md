# Engine checkpoint 19

Real imported-file writes now enter grouped durable saves. The earlier
component metadata exclusion and JSON sizing quotas had made every MUI
request fall back to individual saves. Borrowed allocation accounting now
charges shared component graphs once per cohort, within a 2 GiB capture
allowance and the 5 GB engine working budget. Concrete sizing diagnostics
replace the ambiguous quota failure. The full engine goal remains active.

The immutable runtime pin is OpenPencil
`83e8f1b75f680af828afd605a0546c485a254b7f`, jian
`316d1d0136e011995a30001ead7ee2e9bd66e002`, server SHA-256
`f95902138f6488eaa1d6bd5cacdeb614f72ff6abcff2d888a8c784f0dbc89982`.
The one selective optimized development product build takes 263.5 seconds.
Production types pass; no new unit-test binaries were compiled or executed.

## Real-file grouping evidence

Each fresh case loads its complete original canonical file and offers one
move from each of 32 distinct agents, with at most 32 active requests.

| Complete file | Saved writes / writers | Grouped saves | Service CPU | Case wall |
| --- | ---: | ---: | ---: | ---: |
| MUI, 56,244 nodes | 32 / 32 | 2 | 1.12 s | 2.38 s |
| OpenBridge, 414,531 nodes | 32 / 32 | 2 | 7.36 s | 7.74 s |

Both cases finish exact whole-document field/array/f64-bit/ID verification,
retain their immutable source, and have zero uncertain writes or isolated
fallbacks. The grouped save histograms show one single-write save and one
31-write save. Allocation forecasts include MUI's 999 and Bridge's 13,727
component entries; neither hits a quota. These observations prove activation
and correctness at this offered scope, not 1,000-writer throughput or all
operation families. There is no matched 32-client previous-pin latency claim.

The last measured 1,000-writer MUI case on checkpoint 18 still used individual
saves and took roughly 180 seconds including its outer verifier. Its failed
grouping evidence is retained. The interrupted Bridge run and deferred
validation queue are not passing results and were not replayed.

## Other implemented changes

Lookup now resolves requested component references lazily, avoids duplicate
component-constructor passes, and reuses bounded compact page-location indexes
under the existing shared 32 MiB/64-slot ledger. Fresh APIs remain uncached.
Table preparation avoids duplicate complete JSON-value trees. Additional
explicit authored-layout entry points receive the local reflow marker.

SVG is available through `export_item` and `export_nodes`, with borrowed-page
serialization, physical output scaling and one shared lookup for node batches.
Geometry, ancestor transforms and clips reuse the existing vector serializer.
This closes the transport gap; it does not establish complete SVG feature parity.

PDF scaling is applied. `export_nodes` now defaults to one cropped PDF per node.
`pdfMode: "combined"` returns one multipage file with `nodeIds`, emitted once.
Previously the entire combined PDF was base64-repeated for every requested
node, producing quadratic output growth. Combined node pages retain their own
dimensions. Consumers relying on repeated multipage blobs should choose the
explicit combined mode and read its single aggregate file.

Renderer changes use exact grid-prefix accumulation and skip shadow work only
inside a safely occluding opaque fill. The earlier primitive/corner changes
also remain in source. Pixel outcomes after checkpoint 17 have not been
measured: the user requested larger implementation batches and deferred the
remaining reference queue. The next consolidated reference gate must compare
against measured checkpoint 17; no checkpoint-18 or -19 SSIM improvement is
claimed.

Next implementation work includes broader structural captures/grouping,
headless recovery, streamed native export batches, snapping, actor-owned
history and general atomic tool batches. Existing full import, collaboration,
all-operation and exact whole-file Figma acceptance gaps remain open.

Private evidence: `optimized-engine-19`, `engine-19-consolidated-{mui,bridge}-32`,
`combined-checkpoint-18/implementation-resumed.json`, and the retained earlier
runtime reports. Proprietary designs, fonts and images are not distributed.
