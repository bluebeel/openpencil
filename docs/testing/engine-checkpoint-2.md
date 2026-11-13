# Engine checkpoint 2 — 2026-11-13

Pinned source `62448bb75eb1ececbb778e31950dc7ac40848d29`, same selectively
optimized development profile as the [first checkpoint](optimized-engine-checkpoint.md).
Server SHA-256: `1df7e2f303b3f622e451e849ef983be18bb79c7c16561d6f18fcd37fe4a2d503`.
Exporter SHA-256: `a4e72a1d94183ad21cf4ad243a1603978dd4a5918896590a3ed50123b49a2abd`.

The targeted two-avatar check confirms **zero differing RGBA pixels and
SSIM 1.0 for Material UI**. Park has 16 differing pixels, zero alpha error,
RGB MAE 0.0039876302 and SSIM 0.9999944670. The remaining Park differences
are consistent with image transform precision; no fixture-specific matrix
has been added. These two checks do not replace the final six-case gate.

Request-specific `read_nodes`/`batch_get` replies avoid cloning whole forests;
depth-limited serialization never visits omitted descendants. All 21 query
responses at both 10,000 and 100,000 nodes match the prior normalized response
hashes exactly, including errors, ordering, variables, pages and depth.

| 100,000-node query | Previous median | Checkpoint 2 median |
| --- | ---: | ---: |
| Read three IDs | 39.2 ms | 4.46 ms |
| Read other page | 45.8 ms | 5.19 ms |
| Batch get three IDs | 31.6 ms | 4.45 ms |
| Resolve variable read | 33.0 ms | 0.96 ms |
| Large parent at depth zero | 79.6 ms | 2.11 ms |

Aggregate peak physical footprint fell from 834 to 535 MB. Whole-case
service CPU fell from 4.247 to 1.959 seconds. The unchanged `get_node` control
measured 75–80 ms versus 44 ms before; this checkpoint is not a claim that
every query improved. Four further request-specific lookup paths are being
implemented as the next batch, alongside additional import memory and worker
reuse changes. They are excluded from these pinned measurements.

Local immutable evidence: `optimized-engine-2/build.json`,
`avatar-optimized-engine-2/report.json`, and
`node-query-optimized-after-2/{10000,100000}/report.json`.
The [complete goal](engine-goal-scope.md) remains active; whole-file parity,
cold import below 1 GB and the other operation/concurrency targets are pending.
