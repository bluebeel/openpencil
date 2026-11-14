# Engine checkpoint 17

The combined product builds in 149.2 seconds after production type checks,
before test code generation. Runtime source is
`2e7d52cb4e0e3fad62fd31364b231e10e5c19d77`, jian
`6d1841b223f945da7e8cdacd07e468b8bfe64b53`; server SHA-256 is
`fe02369124f53d9690ebe3f85a6ae57ba325a6e327f74d5b3f15eb8fc0ea49d5`.
This remains the selective optimized development profile. The goal is active.

Glyph meshes choose the shorter quadratic control-hull diagonal and line-only
glyph coverage no longer depends on nearby curved glyphs. Layout preparation
reuses containing-root rectangles across parent queries. Managed ordinary
setters gain a bounded group-save path and explicit authority recovery; every
request retains its own undo step and successful replies follow a saved commit.

Alternate authorities now admit complete files: default source limit 512 MiB,
default retained-state accounting allowance 2 GiB. Optional byte-valued
`OPENPENCIL_MCP_SOURCE_BYTES` and `OPENPENCIL_MCP_RESIDENT_BYTES` tune those
limits, capped at 1 GiB and 4 billion bytes respectively. Invalid/zero values
retain finite defaults. Accounting is not an RSS guarantee, and the primary
document, renderer and request work are additional.

## Results and failures

- All 23 implemented workflows pass on each complete file with 97 acknowledged
  saved mutations, zero uncertain requests and exact final fields/arrays/f64
  bits/IDs. MUI uses 18.30 CPU seconds/149.2 MB RSS; Bridge uses 110.02 CPU
  seconds/849.1 MB. The nine remaining catalog families are not implied covered.
- MUI's 1,000 distinct writers complete 1,000 saved moves with exact canonical
  data in 172.27 CPU seconds/173.67 seconds wall and 176.8 MB RSS. This barely
  changes the previous baseline. Investigation identifies an incorrect group
  admission fallback for reusable component metadata, so grouped throughput on
  these imported files is **not** claimed improved. Its fix is the next batch.
- Both complete files coexist in one service and pass exact saved-data checks
  after one writer per file and ten writers per file. These are 2 and 20 saved
  mutations respectively, not a 1,000-writer multi-file proof. The two-write
  case records 877.8 MB service RSS; the 20-write case uses 17.70 CPU seconds.
- All 12 selected Native/Correlated reference exports verify immutable inputs,
  resource identities and raw comparisons. Correlated MUI differing pixels
  fall from 597 to 164; Bridge update header falls from 56 to 20. Four avatars
  remain raw-zero/SSIM 1. Park and other text-heavy frames remain nonzero.
- Actual node PNG/JPEG/WebP/PDF export passes eight format/scale controls on
  full MUI. This verifies encoded structure and immutable document data, not
  lossy codec pixel identity or complete export-feature parity.
- A newly added full-MUI reflow control finds a real failure: fresh rectangles
  inside an explicitly edited horizontal-layout frame retain x/y zero. The
  document-wide imported-geometry preservation flag currently suppresses new
  authored layout too. A scoped fix is in progress; the failure remains recorded.
- Six real-product HTTP controls pass durable acknowledgement, separate undo,
  actual save failure/rollback, alternate-file isolation, explicit reload and
  process restart. Fourteen focused internal group/recovery controls remain
  unexecuted: the post-product test compiler crossed an overly small external
  5 GB physical-footprint allowance. Compilation stopped after 118.9 seconds;
  independent runtime cases continued. The browser check passes separately.
  This is not a runtime memory failure. The next combined batch will compile
  those controls once with a separate compiler allowance.

The long Bridge 1,000-writer case, remaining format/compositing controls and
final history case are still running/queued when this checkpoint is written.
They have no inferred passing result. Above-1-GB engine use is informational;
the working diagnostic ceiling is 5 GB, with correctness and speed prioritized.

Private evidence: `optimized-engine-17`, `real-document-matrix-17-initial`,
`six-case-engine-17-comparison`, `full-file-authorities-17-{1,10}`,
`extra-exports-17-mui`, `auto-layout-reflow-17-mui`,
`managed-group-public-http-17`, and `engine-batch-17-focused-corrected-budget.json`.
