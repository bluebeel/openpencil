# Engine checkpoint 15

Complete OpenBridge now passes exact canonical verification across 23 bounded
one-client workflows. Repeated MUI PNG export uses 51% less CPU with identical
pixels. Higher-client OpenBridge runs still exceed the 1 GB engine target; those
failures drive the next operation-memory fixes. The full engine goal is active.

## Changes and validation

Persisted style scalars retain source f64 precision, with explicit f32 projection
at layout/paint boundaries. Image-fill bodies are boxed and charged to resident
and history accounting. Canonical nodes retain explicit-null geometry separately
from absent fields, preserving their existing layout defaults. Legacy scalar MCP
setters that parse f32 still have that authoring precision boundary; rich JSON
style input retains f64 precision.

Ordinary MCP edits and undo/redo stop invoking generation heuristics on unrelated
authored geometry. Explicit generation tools repair only their inserted subtrees
or refine target. Metadata history exchanges exact header/key changes; nested
structural entries retain affected nodes/order patches. Full replacement restores
rebase adjacent scoped history without invalidating independent editor clones.
Temporary affected-page capture remains and is a measured memory limitation.

Immutable text coverage is reused by exact path/matrix/clip/sample keys, with
full-key collision checks, repeat admission and eviction. Persistent masks share
an 8 MiB budget with scoped outline/metadata entries **per rendering thread**.
This is not a process-wide 8 MiB bound. Allocation-limit fallback and paint
semantics remain unchanged; active draw references remain working allocations.

159 focused Rust tests and nine harness controls pass, with one pre-existing
ignored Rust test. The selected production/library/test types, actual reference
exporter and browser target check pass. Historical source-including Figma probe
examples fail on missing private modules; those diagnostics are retained and
are excluded from the production gate. Focused test-binary compilation takes
601.4 seconds; browser checking takes 58.5 seconds. These are additional to the
single optimized product build, which takes 261.6 seconds.

The immutable runtime pin is OpenPencil
`b7151e6e1e169c974d8fa29ad5faa9c8d489e046`, jian
`097ad6657bc8bfa12e252ef7fd3012d81e2b6ce1`, server SHA-256
`a9ac9e3fbc12c16b834bad3ef74b9450f833aa3e41bf621498f6f23aa93478af`.
This retains the selective opt-level-3 development profile, not a full release
build. Documentation descendants do not change the pin.

## Complete-file operation results

Ten fresh cases run once. The primary limit now matches the user's stated
scope: shared service plus renderer descendants, sampled every 10 ms and checked
against kernel high-water, below 1,000,000,000 RSS bytes. External generator and
final verifier have a separately reported 5 GB diagnostic allowance. Earlier
strict aggregate failures remain unchanged; they are not relabeled passes.

The matched cases explicitly select the original 16 workflows from the expanded
32-family catalog, at most 32 active requests and 64 global mutation requests.

| MUI logical clients | Durable mutations / writers | Service CPU | Engine peak RSS / physical |
| --- | ---: | ---: | ---: |
| 1 | 20 / 1 | 4.37 s | 150.8 / 122.9 MB |
| 10 | 64 / 10 | 12.20 s | 179.1 / 151.1 MB |
| 100 | 64 / 64 | 12.45 s | 213.7 / 167.1 MB |
| 1,000 | 64 / 64 | 13.90 s | 216.1 / 170.2 MB |

All four retain complete fields/arrays, exact f64 bits and 56,244 IDs. Service
CPU changes against checkpoint 14 are -1.85%, +4.99%, +4.66% and -5.44%:
single observations with mixed timing, not a universal improvement. Larger
counts defer many edit families and do not prove 1,000 successful writers.

OpenBridge one-client original workflows complete all 62 RPCs/20 saved mutations
with 414,531 IDs and exact final fields/arrays/f64 bits, zero uncertain requests,
891.3 MB engine RSS and 867.2 MB physical footprint. Service CPU falls from
34.10 to 25.70 seconds against the same checkpoint-14 engine-scope sequence;
that older sequence failed canonical correctness. Source precision, unrelated
path/stroke rewriting and explicit-null losses are fixed in this complete run.

Separate one-client cases exercise seven additional hierarchy/scalar/theme
workflows on each complete file, including absent-versus-empty map restoration.
Each completes 161 RPCs/77 durable mutations with exact final data and no
uncertain requests. MUI uses 15.25 CPU seconds/134.6 MB RSS; Bridge uses
83.17 CPU seconds/887.6 MB RSS. These seven families are not measured at larger
client counts and exclude arbitrary transformed-group flattening/reference
rebinding claims.

| OpenBridge logical clients | Engine RSS at stop | Acknowledged writes | Uncertain writes | Outcome |
| --- | ---: | ---: | ---: | --- |
| 10 | 1,000.64 MB | 5 | 5 | Resource stop during moves |
| 100 | 1,000.54 MB | 64 | 0 | Resource stop during bounded layout reads; 33 uncertain reads |
| 1,000 | 1,009.16 MB | 8 | 25 | Resource stop during moves |

No stopped case completes final canonical verification or replays uncertain
writes. Attribution shows move targets with one node and layout targets with
at most 29 nodes, on pages up to 109,727 nodes. This supports eliminating
page-wide capture and layout work; it is not an allocator-stack proof or proof
of which pending request was executing when the guard stopped the process.

## Rendering and other operation measurements

Twelve serial exports on one persistent full-MUI service retain the same PNG
bytes as checkpoint 14. Warm exports 3–12 have median wall time 57.90 ms versus
118.09 ms and service CPU 56.98 ms versus 116.88 ms. Peak service RSS is
141.3 MB versus 143.2 MB. First-export wall/CPU is 177.07/143.71 ms versus
180.47/147.55 ms; the second export remains a separate cache-admission sample.
These are one-client results, not a concurrent rendering bound.

All six Native and six Correlated reference exports retain checkpoint-14 PNG
bytes and independently decoded straight RGBA exactly. Input/font/image/oracle
and binary identities verify with zero warnings. Four avatar comparisons remain
raw-zero/SSIM 1. Text/reference errors are unchanged and remain nonzero; the
earlier cold-render cost increase is not claimed solved by a warm-cache result.

Four production 10,000-leaf history/copy workloads pass durable edit/undo/redo
and canonical invariants, retaining all 16 checkpoint-13 PNGs exactly. Service
CPU falls 20.4% for 100 edit/undo/redo operations, 24.7% for ten single-copy
programs, 10.5% for three 100-copy programs and 4.6% for nested copy programs.
RSS changes range from -0.6% to +1.7%. Checkpoint 14 did not rerun these gates,
so checkpoint 13 is the comparison baseline.

## Remaining goal work

Fix large-page edit capture and bounded layout reads; reduce per-write whole-file
saving for many agents; measure more successful writers and active concurrency,
multiple complete documents and remaining operation families. Nine manifest
families remain unsupported/unimplemented, with partial scopes retained in
others. Per-agent undo, conflict/crash recovery, large alternate authorities,
full binary import below 1 GB, Boolean approximations and exact whole-file
Figma pixels remain unfinished. Existing flush/rename saves do not add fsync,
power-loss or journal guarantees. No all-operations or universal memory pass is
claimed.

Private evidence: `optimized-engine-15/build.json`, `engine-batch-15-focused`,
`engine-batch-15-operation-harness`, `real-document-matrix-15`,
`repeated-text-exports-pin15-material-ui-native`,
`six-case-engine-15-comparison`, and `engine-batch-15-root-endgate/comparison.json`.
Original design/font/image inputs are not distributed in this fork.
