# Engine checkpoint 14

Checkpoint 14 reduces durable editing cost on the complete MUI file, fixes
unrelated empty-child mutations, and improves all eight text-heavy reference
comparisons. OpenBridge completes the operation sequence within the service
memory target in a separate control, but its final canonical comparison fails.
The engine goal remains active.

## Implementation and validation

Trusted ordinary commands now capture affected page forests and compact undo
entries into exact changed-node/order patches. Unsupported/global operations
retain the complete snapshot fallback. Undo, redo, abort and failed atomic
batches retain exact optional-field presence and auxiliary state. Capture still
temporarily copies affected forests; this is not constant-memory editing on an
arbitrarily large active page.

Managed saves stream compact JSON with the same typed numeric representation
as the previous host writer, preserving the existing temporary-file, flush and
rename contract. This is process-crash atomic replacement, without a new fsync,
power-loss or journal guarantee. Nine recursive walkers avoid materializing
unrelated absent children. Nested sibling grouping moves complete records, and
multi-ID queries resolve requested nodes in one traversal.

Native text uses analytic quadratic contour coverage with a bounded transient
allocation and the existing fallback for unsupported paths. It preserves the
existing paint/compositing operation; cubic/conic/stroke parity remains separate.

207 focused active tests pass, with one pre-existing ignored test. Production
and browser type checks pass. The single consolidated product build takes
245.8 seconds; focused test/check compilation is additional time. The immutable
runtime source is OpenPencil `458af43c9a867768879d5eaf684f8d334bd3db12` and
jian `eb89f7ad5951716b91458b34f9776a1a73f62dc7`. The server SHA-256 is
`4a2f92af0077cd1282327fab2d225186475adba95032bd85b4e42b9df11719bd`.
This uses the documented selective opt-level-3 development profile, not a
complete release build. Documentation descendants do not change that pin.

## Complete real-document workloads

The matched matrix retains checkpoint 13's command durability, inputs, 27-family
manifest, 64 global mutation budget and maximum 32 active requests. Every MUI
case retains all 71 pages, 56,244 IDs, final fields/arrays and exact f64 bits.
Object-key order is normalized by verification; raw canonical/map-order identity
is not established.

| MUI logical clients | Accepted RPCs | Mutations / writers | Service CPU | Service peak RSS / physical |
| --- | ---: | ---: | ---: | ---: |
| 1 | 62 | 20 / 1 | 4.45 s | 154.6 / 126.9 MB |
| 10 | 231 | 64 / 10 | 11.62 s | 179.7 / 152.0 MB |
| 100 | 947 | 64 / 64 | 11.90 s | 214.7 / 168.3 MB |
| 1,000 | 8,147 | 64 / 64 | 14.70 s | 218.3 / 171.8 MB |

Service CPU falls 54.4%, 56.0%, 54.8% and 46.9% against the respective
checkpoint-13 samples. The previous 10-client sample failed correctness; its
three unrelated empty-child additions are now fixed. The one-client case
exercises all 16 implemented workflows; higher counts defer many edit families.
The 1,000-client result does not establish 1,000 successful writers or 1,000
simultaneous requests.

OpenBridge retains all 18 pages and 414,531 nodes. The strict aggregate guard,
which also includes the external generator, stops PNG export after 19
acknowledged saved edits: 1,007,861,760 bytes aggregate, including 952,107,008
service bytes. One read is unconfirmed; no write is unconfirmed or replayed.
This is a resource stop, not evidence of a renderer crash. Higher client counts
are skipped after the failed prerequisite, and final canonical verification did
not run in this stopped case.

A separate fresh OpenBridge control enforces the user's service-plus-renderer
scope, excluding the external generator/verifier. All 16 workflows finish with
62 acknowledged RPCs, 20 mutations and no uncertain requests. Sampled engine RSS
and kernel high-water both reach 940,359,680 bytes; physical footprint reaches
885,310,952 bytes. This is an observed memory pass for that sequence, with
34.10 seconds service CPU and 35.05 seconds wall time. Its external final
verifier uses more than 2 GB and is reported separately.

That control **fails canonical correctness**: 7,368 numeric values retain f32
bits but change source JSON f64 values; 374 path strings and 18 stroke widths
are rewritten by existing automatic repair heuristics, 374 icon IDs are added,
and 13 explicit-null geometry fields disappear. All IDs/order remain. These
failures are retained without epsilon tolerances, field exclusions or fixture
changes. Memory success does not make this a passing whole case.

## Additional operation and rendering evidence

Five new one-client MUI workflows exercise reparenting, reordering, root grouping,
nested grouping and left/top alignment. All 123 RPCs and 51 mutations finish;
the final complete canonical document retains exact fields/arrays/numeric bits
with only the expected active-page preference. Service CPU is 9.63 seconds,
peak RSS 135.1 MB and physical footprint 113.2 MB. This does not establish
transformed-group visual parity or those families at larger agent counts.
The proposed expanded harness contains 32 families/23 implemented workflows;
variables/themes remain unexecuted. The public matrix still has 27 families.

A request-selection stage control on an actual 8,526-node MUI page preserves
ordered first-match node identities. Selecting 1,000 IDs falls from 20.49 to
0.291 ms (70.4 times faster). This is isolated wall duration, without per-stage
CPU or agent/RPC-throughput inference.

All six Native and six Correlated exports verify input/font/image/oracle and
binary identities, with zero warnings. Four avatar comparisons remain exact
PNG and straight RGBA, SSIM 1. All eight text-heavy comparisons improve RGB
absolute error and SSIM. MUI Welcome Native error falls 39.1% to 166,367 and
SSIM reaches 0.9997978782; Correlated error falls 44.2% to 125,777 and SSIM
reaches 0.9998070478. Park Native error falls 5.4%; OpenBridge header alpha
error falls from 6,630 to 3,811 Native and 5,815 to 3,445 Correlated.

Cold render-phase wall duration increases in all eight text cases: MUI Native
131 to 192 ms, Park 651 to 823 ms, and Bridge cover 3,754 to 4,977 ms. These
are single cold samples, excluding queue wait, not timing distributions or
isolated CPU percentages. The selected Bridge exports peak at 804.0 MB RSS;
selected frames do not substitute for the full-document operation matrix.

## Remaining acceptance work

Preserve source precision/null presence and scope automatic repairs; reduce
repeated text coverage cost; expand variables/themes and other operation
families; measure more successful writers, active concurrency and multiple full
files. Full binary import still needs its separate 5 GB diagnostic allowance.
Large alternate-file authority, per-agent undo, conflict/crash recovery,
137 imported Boolean approximations and exact whole-file Figma pixels remain
incomplete. No universal 1 GB or complete operation/parity claim is made.

Private evidence: `optimized-engine-14/build.json`, `engine-batch-14-focused`,
`real-document-matrix-14/comparison-13-to-14.json`,
`real-document-operations-pin14-openbridge-engine-scope-1/final-control-report.json`,
`real-document-families-pin14-material-ui-1/summary.json`,
`engine-batch-14-query-selection/stage-control/report.json`, and
`six-case-engine-14-comparison/compact.json`. Original design/font/image inputs
are not distributed in this fork.
