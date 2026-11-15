# Checkpoint 13: compact documents and exact durable edits

The measured source is OpenPencil `a894875c7e004fdee7c14c3299d3bd6c1dfd948b`
with Jian `7631bfc1c2edd5a14c6a13a0f1b7b540c9b0584c`. One consolidated
optimized development build completed in 80.89 seconds. This is the same
selectively optimized profile as checkpoint 12, not a full release build.
Source remained clean and unchanged during the build. Validation and its
incremental fixture corrections took additional time; 80.89 seconds is the
product build duration, not the complete development cycle.

## Changes

- Allocate optional events, lifecycle, semantics, gestures, widget states and
  transitions only when present. Preserve absent, empty and populated wire
  values; account for owned boxes in live and shared-history memory ledgers.
- Stream eligible canonical files into typed data without retaining a complete
  JSON DOM. Share compatibility policy with the established fallback, including
  legacy, duplicate-field, malformed, editor-metadata and warning behavior.
- Preserve exact f64 parsing, including signed zero. Production file/source
  loading and atomic saving retain all 1,852 numeric regression properties.
- Route structured MCP numeric geometry through canonical patches when the
  legacy integer command cannot carry its spelling/value. Fractions, `3.0`,
  negative zero and out-of-i32 integers work; integer literals retain their
  lightweight route and legacy numeric-string behavior is unchanged.
- Borrow leaf metadata during JSON transforms instead of cloning it first.
- Save bound Managed primary command changes before their version/SSE tick and
  successful acknowledgement. Failed saves restore the checkpoint and Cut
  clipboard; authority handoffs refuse before mutation.

The save contract uses the existing flushed sibling-file/atomic-rename writer.
It does not add fsync/power-loss guarantees. Local/Online/unbound sessions and
separate REST/direct filesystem mutators retain their existing contracts.
Managed acknowledgements now include whole-document saving, so their CPU and
latency cannot be compared directly with checkpoint 12's live-only replies.

## Complete real-file service cases

All inputs are immutable complete conversions: MUI has 71 pages/56,244 nodes;
OpenBridge has 18 pages/414,531 nodes. The guard allows 1,000,000,000 aggregate
RSS bytes, including the load generator. Service memory is recorded separately.
Imported font files and binary/source hashes are pinned.

| Full MUI case | Accepted RPCs / mutations / mutation agents | Service peak RSS / physical | Outcome |
| --- | --- | --- | --- |
| 1 logical client | 62 / 20 / 1 | 381.9 / 290.1 MB | Exact final fields and f64 bits pass; 16 implemented families exercised |
| 10 logical clients | 231 / 64 / 10 | 276.1 / 248.5 MB | Correctness failure: three unrelated childless frames gain `children:[]` |
| 100 logical clients | 947 / 64 / 64 | 299.1 / 273.3 MB | Exact final fields and f64 bits pass; most edit families budget-deferred |
| 1,000 logical clients | 8,147 / 64 / 64 | 303.3 / 277.5 MB | Exact final fields and f64 bits pass; most edit families budget-deferred |

The first MUI case previously peaked at 889 MB service RSS. The new case uses
381.9 MB while saving each acknowledged command. MUI service CPU totals are
9.77/26.43/26.30/27.68 seconds for the four rows; persistence/work counts differ
from checkpoint 12, so these totals are not a matched throughput comparison.

OpenBridge now completes startup in 2.592 seconds and accepts read/search/UI
calls with all nodes present, reaching 830.5 MB observed service RSS and
695.2 MB physical footprint. Its first move exceeds the strict guard:
aggregate RSS reaches 1,075.8 MB and the largest process reaches 1,022.5 MB.
There are eight confirmed read/context replies, no confirmed mutation reply,
and one unconfirmed update. The workcopy remains unchanged and no save sibling
exists. This is a first-edit/pre-save failure; an allocation stack was not
captured. Full-history capture is the next implementation target.

The three larger OpenBridge cases are explicitly unrun after this failed
prerequisite. No uncertain mutation is replayed. All cases use at most 32
active requests and a global 64-mutation budget: 1,000 logical clients do not
mean 1,000 simultaneous requests or 1,000 successful writers. Object-key order
is normalized by the final semantic comparison; raw canonical byte/map-order
identity is not proven. The MUI 10-client failure remains a failure, with all
three unexpected additions retained for diagnosis.

## Matched generated workloads and reference images

Four production history/copy gates retain all 16 checkpoint-12 PNGs exactly,
their final document comparisons and durable edit/undo/redo invariants.

| Workload | Service CPU change | Service peak RSS change |
| --- | ---: | ---: |
| 100 edits/undo/redo on 10,000 leaves | -6.9% | -34.1% |
| Ten single-copy programs | +6.1% | -37.9% |
| Three 100-copy programs | -4.5% | -36.8% |
| Three nested 100-copy programs | -8.2% | -38.3% |

These are single matched samples, with mixed timing results. No universal
speed improvement is claimed.

All six Native and six Correlated reference exports retain checkpoint-12 PNG
bytes and independently decoded straight RGBA exactly. Input/font/image/oracle
identities pass, with zero warnings. Both avatar targets remain raw-zero/SSIM
1; MUI's remaining paragraph/underline/glyph differences remain. Selected
OpenBridge exports peak at 806.7 MB RSS/797.1 MB physical; this is selected-frame
coverage, not full-document rendering or import memory coverage.

## Validation and remaining work

122 focused tests pass: 52 service persistence/accounting, 23 core history/JSON/
transaction, 13 loader, 19 schema, one AOT, two precision, six managed commit
and six numeric MCP cases. Production compilation and the browser target check
pass. Earlier fixture mistakes and the decimal-routing failure are retained in
the private diagnostic artifacts; only affected cases were rerun.

The goal remains active: fix first-edit history memory and the three unrelated
empty-child mutations, reduce durable-write cost, expand the remaining 27-family
manifest, and measure higher active concurrency and complete large alternate
files. Per-agent undo, conflict/crash recovery and full Figma feature/pixel
parity remain incomplete. Full binary import retains its separate 5 GB diagnostic
allowance; no full import below 1 GB is claimed.

Private evidence: `optimized-engine-13/build.json`, `engine-batch-13-focused`,
`real-document-matrix-13`, `engine-batch-13-root-endgate/comparison.json`, and
`six-case-engine-13-comparison`. Original design/font/image fixtures are not
distributed in this fork.
