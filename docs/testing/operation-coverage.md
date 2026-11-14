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
- At checkpoint 11 the inline `PenNode` was 2,696 bytes. OpenBridge's 414,531 nodes require
  1,117,575,576 bytes for these values alone, before strings/assets/vector spare
  capacity. Compact node storage or lazy page ownership is required for 1 GB.
- The full OpenBridge import preserves 545 warnings, including 137 boolean-shape
  approximations. Exact full-document Figma feature parity is incomplete.

The existing reports do not prove all operations at all agent counts on the
complete real files. The 1 GB runtime samples do not establish a universal memory
bound. Full OpenBridge conversion already contradicts such a broad claim.

## Checkpoint-12 real-file outcomes

The new [real-document harness](../../scripts/bench/real-document-operations.py)
and [27-family coverage manifest](../../scripts/bench/real-document-operations.json)
ran 11 fresh cases and recorded three startup-prerequisite skips. All workloads
use at most 32 active requests; 100/1,000-active stress is still unmeasured.

| Full document / scope | 1 client | 10 clients | 100 clients | 1,000 clients |
| --- | --- | --- | --- | --- |
| MUI, strict aggregate 1 GB, live + final save | 20 mutations; numeric-bit mismatch | 102 mutations; aggregate limit during undo/redo | 931 mutations; aggregate limit during undo/redo | 1,000 moves; other edit families deferred; numeric-bit mismatch |
| Bridge, strict aggregate 1 GB | Startup limit | Not run: startup prerequisite failed | Not run: startup prerequisite failed | Not run: startup prerequisite failed |
| Bridge, 5 GB read/search/UI diagnostic | Pass for subset | Pass for subset | Pass for subset | Pass for subset |

MUI's completed 1/1,000-client cases use 2.20/41.80 service CPU seconds and peak
889/712 MB service RSS (937/803 MB aggregate). Different operation participation
makes these unsuitable as an equal-work scaling comparison. The 10/100-client
stops observe service processes at 964/982 MB while aggregate RSS exceeds 1 GB;
these do not prove service-only violations or completed 1 GB passes.

Bridge read/search/UI diagnostics use 3.67/3.16/3.39/4.93 service CPU seconds and
2,637/2,733/2,731/2,638 MB peak service RSS across the four counts. They issue no
document mutation or save. The diagnostic allowance is not a 1 GB result.

Separate MUI save-after-each-write cases run only 1/10 clients with a 16-mutation
budget. One client stops during redo; ten clients save 16 mutations but fail
strict numeric comparison. Three stopped-run commands remain unconfirmed and
were never replayed. All three completed MUI outputs retain all 56,244 IDs and
intended edits, but 1,852 unrelated f64 values drift one ULP, with unchanged f32
bits, plus the requested active-page preference. No epsilon or blanket metadata
exclusion converted failures to passes. Object map order is unverified because
the semantic fingerprint sorts keys.

The live catalog has 179 tools. Availability is distinct from execution; every
unexercised family, resource/budget deferral and missing transport remains in the
coverage manifest. Full per-phase CPU, memory, latency, participants and intent /
acknowledgement journals remain in private `real-document-matrix-12/` reports.

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

At checkpoint 12 the live primary MCP mutation path required explicit
`save_document` for disk persistence; its two-RPC durable workflow was not an
atomic acknowledgement. Checkpoint 13 adds save-before-ack and rollback for
command writes in bound Managed primary sessions. Alternate-file authorities
retain their own save-before-ack behavior. Local/Online/unbound sessions and
separate REST/direct filesystem mutators retain their previous contracts;
all-route durability, journals and power-loss recovery remain unfinished.

Operation families requiring explicit coverage include drawing/transforms,
selection/hit testing/snapping, layout/reflow, vector editing/booleans,
compositing, typography, component/variable dependencies, queries, pan/zoom and
redraw, image/SVG/raster/PDF interchange, persistence/recovery and collaboration.
The new real-document harness and tool inventory preserve partial and failed coverage explicitly. Operations
without a working engine/MCP surface still require implementation.


## Checkpoint-13 real-file outcomes

[Checkpoint 13](engine-checkpoint-13.md) compacted optional metadata and streamed
canonical file loading. Managed bound primary command replies now include atomic
saving, so live-only checkpoint 12 latency is not a matched baseline.

| Full file, strict aggregate 1 GB | 1 client | 10 clients | 100 clients | 1,000 clients |
| --- | --- | --- | --- | --- |
| MUI | Exact fields/bits pass; 20 writes | Failure: 3 unrelated `children:[]` additions; 64 writes | Exact pass;64 writers, other edit families deferred | Exact pass;64 writers, other edit families deferred |
| OpenBridge | Full open/read/search/UI pass; first move exceeds RSS guard, unconfirmed | Not run after failed write prerequisite | Not run after failed write prerequisite | Not run after failed write prerequisite |

MUI service peak RSS is 381.9/276.1/299.1/303.3 MB. OpenBridge loads all 414,531
nodes but its first update reaches 1,022.5 MB in the largest process. Maximum active
requests remains 32; 64 mutation requests is the global per-case budget. These
results do not cover all 27 families or 1,000 writers. The 10-client failure is real
None-to-Some(empty) mutation, not numeric drift or a relaxed comparison.
