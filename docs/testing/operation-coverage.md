# Engine operation benchmark coverage

Status: incomplete. The full [engine goal](engine-goal-scope.md) includes every
listed operation family and complete OpenBridge/MUI documents at 1, 10, 100 and
1,000 agents. Logical clients, offered concurrency, admitted work and successful
operations are separate measurements. A 32-request window is not evidence for
1,000 simultaneous operations.

## Authoritative measurements before checkpoint 12

| Dataset / workload | Scope actually exercised | Clients / active requests | Evidence |
| --- | --- | --- | --- |
| Generated flat/nested 10,000-node documents | Selected serial move, resize, duplicate, text, effect, component, SVG, image, clipboard, viewport and undo/redo cases; 13 invariants | 1 / 1 | Checkpoint 10 operation matrices |
| Generated shared document | Two rounds of four-node drawing batches, 8,000 edits | 1,000 / 32 | Checkpoint 11 single-file context gate |
| Three generated shared documents | Same drawing workload plus six UI contexts and cross-file invariants | 1,000 / 32 | Checkpoint 11 multi-file context gate |
| Generated 10,000-leaf document | Standalone 100-edit history and selected mixed copy/move/delete programs | 1 / 1 | Checkpoint 11 history/copy gates |
| Generated 101,000-node document | 1,000 PNG file exports | 1 / 1 | Checkpoint 10 bulk export gate |
| Full MUI conversion | 71-page CLI and HTTP import; exact canonical data except intended new ligature features | 1 conversion | Checkpoint 11 import gate |
| Selected real Figma frames | Six fixed Native and six Correlated reference exports with immutable fonts/images | 1 export process at a time | Checkpoint 11 rendering gate |
| Full OpenBridge conversion | All 18 pages, immutable pin11 CLI, 205,742,550-byte canonical output | 1 conversion | New full-fixture diagnostic; 2,790,801,408-byte RSS exceeds 1 GB |

New full-primary calibration adds the following limited evidence:

- MUI selected read/search/context/move/resize/text phases ran at one agent;
  the first attempt stopped on a harness fill-normalization assertion. Peak
  service RSS was about 645 MB. This is partial coverage, not a passed matrix.
- Full OpenBridge startup exceeded the strict 1 GB RSS guard before handshake.
  One separately labeled 5 GB diagnostic completed cold open/read/search:
  startup 3.648 seconds, peak RSS 2.890 GB, sampled physical 2.063 GB and steady
  read/search about 2.26 GB RSS / 1.65 GB physical. No writes or larger client
  counts ran in that diagnostic. Original fixture bytes were unchanged.
- The current inline `PenNode` is 2,696 bytes. OpenBridge's 414,531 nodes require
  1,117,575,576 bytes for these values alone, before strings/assets/vector spare
  capacity. Compact node storage or lazy page ownership is required for 1 GB.
- The full OpenBridge import preserves 545 warnings, including 137 boolean-shape
  approximations. Exact full-document Figma feature parity is incomplete.

The existing reports do not prove all operations at all agent counts on the
complete real files. The 1 GB runtime samples do not establish a universal memory
bound. Full OpenBridge conversion already contradicts such a broad claim.

## Required real-file matrix

Use complete imported documents, immutable resource identities and actual
imported-node worksets. Record cold/warm wall time, p50/p95 latency, process CPU,
throughput, queue/rejection behavior, sampled RSS/physical memory and kernel RSS
high-water. Separate service/renderer memory from load-generator memory. Preserve
no-loss, ID, canonical document, transaction, UI-context, clipboard, persisted
state and rendered-resource invariants.

Run 1, 10, 100 and 1,000 logical clients, and report the active request window.
Exercise requested concurrency separately from bounded admitted concurrency.
Unsupported features, missing MCP transport, over-budget cases, explicit
refusals and timeouts remain visible in coverage; they are not passing samples.

The full input documents currently exceed the alternate-file authority's 16 MiB
source cap: MUI is 26,682,248 bytes and OpenBridge 205,742,550 bytes. Single-file
primary-open measurements and alternate-file refusals therefore require separate
cases. Do not trim inactive pages or assets to make the full-file matrix pass.

The live primary MCP mutation path currently requires explicit `save_document`
for disk persistence. The new harness labels live-final-save and explicit
save-after-each-write workflows separately; the latter uses two RPCs and does
not establish an atomic durable acknowledgement. Alternate-file authorities
have separate save-before-ack behavior. Primary atomic durability remains an
unfinished goal requirement.

Operation families requiring explicit coverage include drawing/transforms,
selection/hit testing/snapping, layout/reflow, vector editing/booleans,
compositing, typography, component/variable dependencies, queries, pan/zoom and
redraw, image/SVG/raster/PDF interchange, persistence/recovery and collaboration.
The new real-document harness and tool inventory are in progress. Operations
without a working engine/MCP surface still require implementation.
