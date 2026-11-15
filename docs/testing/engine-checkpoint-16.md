# Engine checkpoint 16

Targeted command capture replaces unrelated page-forest cloning for proven
ordinary edits, sparse strokes use boxed storage, bounded parent layout reads
retain complete layout inputs, and horizontal glyph/decorations use consistent
edge ownership. This checkpoint does not complete the engine goal.

The runtime pin is OpenPencil `dbaf76613f3b3953e5481e3f8c4ce13b39c49d2b`,
jian `6d1841b223f945da7e8cdacd07e468b8bfe64b53`, server SHA-256
`f5675ff70f02b73559c5ac306d6a9f0c8c5389056dfc18f674f736fa43616ab5`.
One selective opt-level-3 development product build took 260.8 seconds.
Production/test types and browser checks pass; 203 unique focused Rust checks
pass, with one pre-existing ignored test. The focused test binaries took an
additional 646.1 seconds to compile. Future batches build the product after
type checking, before focused test code generation.

## Complete files

Both complete files preserve every canonical field, array, f64 bit and node ID
across 23 bounded workflows in separate original-16/additional-seven one-client
cases. MUI contains 56,244 nodes; OpenBridge contains 414,531.

| MUI logical clients | Saved writes / writers | Service CPU | Engine peak RSS |
| --- | ---: | ---: | ---: |
| 1 | 20 / 1 | 4.52 s | 143.9 MB |
| 10 | 64 / 10 | 12.60 s | 167.1 MB |
| 100 | 64 / 64 | 12.22 s | 188.0 MB |
| 1,000 | 64 / 64 | 13.81 s | 189.2 MB |

The matched larger-client cases cap active requests at 32 and global writes at
64. Their writer participation is not 1,000. Compared with checkpoint 15,
MUI peak RSS falls 4.6–12.5%; CPU changes are mixed, approximately -1.9–+3.5%.

OpenBridge original-one-client completes 20 writes at 904.5 MB RSS and
26.60 CPU seconds. Its separate additional-seven case completes 77 writes at
829.4 MB and 84.95 CPU seconds. Strict 10/100/1,000-client cases each acknowledge
all 64 offered writes, then stop during layout at 1,004.4/1,001.9/1,008.2 MB.
There are zero uncertain writes throughout this matrix, versus 30 at checkpoint
15. Stopped cases retain uncertain reads and have no final canonical proof.

Under the subsequently authorized diagnostic policy, a fresh OpenBridge
10-client case completes all 231 RPCs, all 64 saved writes and final exact
canonical verification at **1,052.4 MB** RSS, 72.01 CPU seconds and 74.59 seconds
wall time. The 1 GB result remains a recorded failure of that optimization
target; it does not prevent completion under the 5 GB working ceiling. Ten
layout reads in this fresh case take 0.718 seconds wall/0.412 CPU seconds;
the sampled 1 GB crossing occurs later during image export. Allocation history
differs between fresh cases; the strict failures remain unchanged.

A separate full-MUI move-only case completes **1,000 saved moves from 1,000
distinct writers**, all 2,003 RPCs, zero uncertain requests, and exact final
canonical verification. It uses 173.5 MB engine RSS, 172.91 CPU seconds and
177.24 seconds wall time, with at most 32 active requests. Throughput is about
5.64 saved moves/second. This establishes a real writer-count baseline for
durable group commits; it is not 1,000 simultaneous requests or all operations.

## Rendering and history

All six Native and six Correlated exports complete with verified immutable
inputs/fonts/images/oracles and zero identity errors or warnings. Four avatar
comparisons remain straight-RGBA exact and SSIM 1. Text-heavy output changes
from checkpoint 15. Correlated MUI Welcome has 597 differing straight-RGBA
pixels; Bridge update header has 56; Park cover still has 37,365. These are
selected frames, not whole-file parity. Native and Correlated results remain
separate; no whole-file SSIM 1 claim is made.

Twelve repeated full-MUI PNG exports are internally identical. Warm median
wall/CPU time is 61.19/59.78 ms; checkpoint 15 is 57.90/56.98 ms. Changed text
coverage intentionally changes the PNG, and this observation is not presented
as another speed improvement. Peak service RSS is 136.3 MB.

All four production 10,000-leaf history/copy cases pass persisted-state and
within-case original/undo and edited/redo PNG invariants. History CPU falls from
7.19 to 6.25 seconds; the three copy cases remain approximately unchanged.
An initial driver retained checkpoint-15 destination names and correctly
refused to overwrite old results. A distinct corrected driver ran the four
previously unexecuted cases; the original failed attempt remains recorded.

## Next priorities

Correctness, operation coverage and speed take priority over the 1 GB target,
as explicitly clarified by the user. Continue within a 5 GB working ceiling;
record memory, complete independent cases, and fix slow operations without
using the optimization target as a release gate. Reuse prepared layout across
parent queries, group durable writes, admit complete alternate files and close
remaining glyph errors. Nine catalog families and partial scopes remain
unfinished, along with per-agent undo, larger multi-file workloads, complete
failure recovery, full binary-import memory and whole-file rendering parity.

Private evidence: `optimized-engine-16/build.json`, `engine-batch-16-focused`,
`real-document-matrix-16`, `real-document-diagnostics-16`,
`six-case-engine-16-comparison`, `repeated-text-exports-pin16-material-ui-native`,
and `engine-batch-16-root-endgate-corrected`. Proprietary source assets are not
distributed in the fork.
