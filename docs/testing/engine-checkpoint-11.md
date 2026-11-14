# Batched engine checkpoint 11 — 2026-11-14

The batch combines copy correctness, ID validation, common history capture and
text parity before one selectively optimized product build. The profile matches
checkpoint 10. The full [engine goal](engine-goal-scope.md) remains active.

## Changes

Deep copying now clones each node's metadata once and recursively copies its
children, removing repeated complete-subtree clones at every depth. Copying and
ID remapping preserve absent children: component instances with no override
continue inheriting their master's content. Explicit empty overrides remain
empty. Focused tests verify inherited paint, depth-first IDs and exact undo/redo.

Authored subtree commands do not request an unused sequential allocator seed.
They remain available when unrelated numeric IDs exhaust that allocator. Incoming
IDs are validated and deduplicated once. A complete borrowed scan of all page,
root and descendant IDs proves fresh numeric forests cannot collide; custom IDs,
nonfresh IDs and root-placeholder reuse retain full collision checks. Mixed MCP
copy programs likewise reserve only the incoming forest after proving the exact
current document high-water. Public mutable documents are still rescanned.

History capture compares common rectangle leaves through schema-exhaustive typed
fields, preserving numeric bits, map order and absent/empty distinctions. Rich or
unsupported values retain the previous exact comparison and sensitivity fallback.
Actual private 2,000-by-four-insert controls improve CPU time 16–17%, with every
canonical state and 100 undo/redo checkpoints exact. This is a stage measurement;
end-to-end results follow below.

Text import maps authored common-ligature flags to the existing `liga` feature.
Saved joined Latin clusters can recover a linked style's enabled ligatures when
a stale direct flag conflicts with the saved geometry. Explicit overrides and
OpenType OFF remain protected. Candidate evidence is bounded to 64 retained
spans across the full UTF-16 text. Native authored Latin spacing advances per
shaped cluster, including ligatures, rather than per source character. Five saved
MUI paragraph lines match the staged glyph positions exactly; final PNG results
are evaluated separately. Mixed per-span feature maps remain unsupported and
explicit span OFF conservatively disables the whole node.

## Validation

Focused groups pass 33 ID/batch, 3 copy, 80 MCP and 23 history test executions;
groups can overlap and two pre-existing heavy tests remain ignored. Text/importer
filters pass 74 executions. A new 96-level copy test exceeded the default Rust
test-thread stack; its 16 MB control passed. The committed test uses 32 levels
and shallow metadata comparisons, and passes with the default stack. Only the
affected copy filter was refreshed after that fixture correction.

Unchanged bulk-export, operation-matrix, JavaScript and legacy browser suites
were not repeated in this batch. The runtime gate targets changed paths: history,
copy programs, concurrent editing, all-pages text imports and final rendering.

## Image interpolation investigation

No image shader changes are included in this checkpoint. A source-derived
triangle setup and edge-ownership model reproduces all 1,268,220 captured UV/local
Float32 components across 317,055 fragments and 24 camera controls, including
rotation, reflection and anisotropic scales. Actual standalone Skia evaluation
matches too. Native per-camera plane construction and frozen-photo end-to-end
controls remain pending. This stage result does not establish final image parity.

## Runtime evidence

Measured source: `77975fb24674554b3d72dcdd8088ff88ea6da942`; Jian:
`ef992bc2de292dec744eeb9181621b1c270b8e99`. One clean combined build took
113.37 seconds. Binary identities and exact profile are retained in
`optimized-engine-11/build.json`.

Four selected history/copy cases pass every acknowledged canonical document,
undo/redo and all 16 restored PNG byte comparisons against checkpoint 10. The
10,000-node history workload takes 13.490 seconds / 9.485 service CPU seconds,
versus 13.730 / 9.735. The three 301-command copy programs take 4.937 seconds /
4.753 CPU seconds versus 5.514 / 5.307 (10.5% less CPU). A matched new 24-level
nested-copy control takes 4.995 seconds / 4.794 CPU seconds versus 5.458 / 5.265
(8.9% less CPU). Ten single-copy programs are essentially unchanged: wall time
rises 1.2%, CPU falls 0.5%. These are single matched samples.

The largest service physical footprint across those four cases is 176.2 MB,
kernel child RSS 218.1 MB and aggregate RSS 274.1 MB, all under the strict 1 GB
runtime guard. The service/renderer budget excludes external AI models; the
runtime guard also includes the load generator.

Both 1,000-logical-client / 32-active-request context runs pass 2,000 accepted
requests and 8,000 edits each. Single-file edit time falls 2.702 to 2.254 seconds
and service CPU 3.142 to 2.662 seconds (15.3%); three-file edit time falls 3.051
to 2.960 seconds and CPU 6.934 to 6.680 seconds (3.7%). Single-file total wall time
rises 3.525 to 4.264 seconds, with observed startup 0.014 to 1.189 seconds; it
was not rerun. No total-wall speedup is claimed. Maximum service physical memory
is 190.3 MB, kernel RSS 214.6 MB and aggregate RSS 262.8 MB. All four PNGs and
full name-keyed data/geometry match checkpoint 10 after the established
ID/order and absent/empty-child normalization. Six multi-file UI contexts remain
isolated; UI-only operations preserve durable bytes. These are bounded active
requests, not 1,000 simultaneous renderers.

A fresh service accepts 1,000 distinct agent IDs, then spends 3.000 seconds with
no requests. CPU advances 0.0004245 seconds (0.0141% of one core); document,
version and durable hash stay unchanged. Idle service physical memory is
11.5 MB / RSS 24.9 MB. Successful distinct IDs are verified directly; HTTP still
does not expose an actual resident-context count.

The six Native and six Correlated exports pass all resource, input, font, image,
reference and runner identity checks with no warnings. All ten outputs outside
MUI Welcome retain checkpoint-10 PNG bytes and independently decoded straight
RGBA exactly. Park and MUI avatars remain exact Figma matches in both modes.

| MUI Welcome | Checkpoint-10 SSIM | Checkpoint-11 SSIM | Raw RGB MAE before / after | Changed RGBA pixels before / after |
| --- | ---: | ---: | ---: | ---: |
| Native | 0.998585730 | 0.999694654 | 0.156302 / 0.093531 | 21,309 / 20,613 |
| Correlated | 0.998534518 | 0.99964459 | 0.145703 / 0.082898 | 7,370 / 6,622 |

RGB error falls 40.2% / 43.1%; full-frame exact parity is not yet reached. Native
MUI rendering takes 127.9 ms and Correlated 593.3 ms, approximately unchanged.
Park takes 564.3 / 3,445.5 ms versus 567.6 / 3,457.8 ms. All four OpenBridge cold
processes remain below 1 GB: maximum physical footprint 794.7 MB and RSS
804.2 MB. No universal renderer speedup is inferred from these single samples.

Production CLI and HTTP all-pages MUI imports pass exact route-matched canonical
comparison against pin10, permitting only newly mapped `liga` tags. CLI adds
7,790 OFF and 2,282 ON tags; HTTP adds 7,808 OFF and 2,291 ON tags. Existing
feature values and every other field, geometry, numeric value, ordering and
unknown property remain exact. Both retain 71 pages, all IDs and 56 images /
120 references; source bytes and the HTTP authority/version remain unchanged.
Both children verify the exact source/Jian/binary pin before process creation.

CLI conversion takes 3.786 seconds / 3.352 CPU seconds, with 445.7 MB sampled
physical memory and 522.2 MB kernel RSS. HTTP takes 3.893 seconds / 3.565 service
CPU seconds, with 462.1 MB service physical memory and 693.6 MB kernel RSS.
Imports use the authorized 5 GB allowance. The HTTP harness's aggregate physical
peak is 955.6 MB; these results do not establish a universal 1 GB import bound.

## Evidence and remaining work

Private reports remain outside the public fork:

- `optimized-engine-11/build.json`
- `engine-batch-11-core-final/final-summary.json`
- `history-typed-batch11-report.json`
- `text-parity-batch11/report.json`
- `engine-batch-11-root-endgate/comparison.json`
- `runtime-contexts-optimized-11-agent-checks.json`
- `runtime-imports-optimized-11-agent-checks.json`
- `six-case-engine-11-comparison/{report,summary}.json`
- `image-varying-batch11-proof/report.json`

Larger frames still differ from Figma. The image camera adapter and frozen-photo
proof, remaining text contour/underline/dither differences, repeated whole-tree
ID scans and subtree JSON conversion remain optimization targets. Global memory
admission, selective per-agent undo, higher active concurrency, and broader
cancellation/idempotency/durability coverage remain incomplete. The full goal
stays active.
