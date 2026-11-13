# Batched engine checkpoint 3 — 2026-11-13

Measured source: `f1dde782b7836bf3db7fb9f904d75342bf02732f`.
Same selectively optimized development profile as the earlier checkpoints;
this is not a full release-profile measurement.

Server SHA-256: `579dcdbab3a3643b3a36adc993ce81e1c06fc2c942003a4e1c0fa8aa9ad91472`.
Exporter SHA-256: `cd2fd0918654ca0111b415ea073d79441f3df7c5b3612f511663068e2dfe8b2f`.
The source stayed clean and unchanged during the 125.5-second combined build.

## Changes and validation cadence

This batch combines request-specific node, parent, children, name and
selection reads; exact vector/color import packing; corrected text-mask
sampling; persistent bounded HTTP workers; and reusable clean save buffers.
Legacy query constructors and the public selection struct remain compatible.
Worker demand and queues remain bounded; completed saves retain no file
handle, and errors/panics discard buffered bytes before another save.

Focused checks ran during implementation: 69 MCP query tests, 329 importer
tests, six native text tests and the relevant host persistence, admission,
worker, save-buffer, agent-context and shutdown checks. The host test binary
was compiled once; the complete 1,503-test host suite was not rerun.
One optimized build and one six-case rendering gate closed the batch.

## Shared service measurements

One service, 1,000 logical agents, 32 requests maximum in flight, 2,000 MCP
mutation requests and 8,000 drawing edits. Each agent uses its own `agentId`.
All expected names, unique IDs, geometry, acknowledged counts and saved state
passed, with real PNG exports and no ambiguous write retries.

| Workload | Earlier optimized edit time | Batch 3 edit time | Batch 3 service CPU | Aggregate peak physical memory |
| --- | ---: | ---: | ---: | ---: |
| One shared file | 2.961 s | 1.526 s | 2.017 s | 287 MB |
| Three resident files | 9.985 s | 5.214 s | 11.201 s | 300 MB |

CPU covers the whole case. Service-only physical peaks were 103/115 MB.
Single-file in-flight P95 request latency fell from 109 to 40 ms. The
three-file case also preserved separate agent/document selections and cameras,
and UI-only calls left saved bytes unchanged. Its aggregate memory increased
from 266 to 300 MB; reusable worker/save storage trades a bounded amount of
retained memory for less per-request allocation and thread creation.

Aggregate measurements include the load generator, exclude external models,
and sample every 50 ms. They are not allocator hard limits or evidence for
unlimited offered load. These are workload measurements of a combined batch,
not isolated causal timings for each individual change.

## Large-document queries

All 33 query cases at both 10,000 and 100,000 nodes return the exact prior
normalized response hashes, including error replies, duplicate-ID policies,
ordering, variables, pages and selection depth.

| 100,000-node operation | Prior median | Batch 3 median |
| --- | ---: | ---: |
| Get node | 58.39 ms | 3.58 ms |
| Get parent | 12.25 ms | 1.82 ms |
| Get children | 50.84 ms | 3.03 ms |
| Find first exact name | 15.31 ms | 0.358 ms |
| Selected 49,999-node subtree, depth zero | 18.69 ms | 0.338 ms |
| Invalid selection depth | 22.37 ms | 0.326 ms |

Whole-case service CPU fell from 3.252 to 0.577 seconds; wall time fell from
3.624 to 0.675 seconds. Aggregate physical peak fell from 719 to 534 MB.
The 10,000-node case improved per-query timings and CPU, but its total wall
time increased from 0.412 to 1.648 seconds during cold startup. No whole-case
10,000-node wall-time gain is claimed.

## Rendering and import memory

All six comparisons improved against the first optimized six-case checkpoint.
Material UI's avatar remains exact: zero differing RGBA pixels, zero RGB/alpha
error and SSIM 1.0. Park's avatar has 16 differing pixels and zero alpha error.

| Case | Batch 3 SSIM | RGB MAE |
| --- | ---: | ---: |
| Park cover | 0.9962054089 | 0.3859505 |
| Material UI welcome | 0.9974294577 | 0.2217489 |
| OpenBridge cover | 0.9998478594 | 0.0394362 |
| Park avatar | 0.9999944670 | 0.0039876 |
| Material UI avatar | 1.0 | 0 |
| OpenBridge header | 0.9993980621 | 0.0328741 |

Exact vector/color packing saves 123,224,256 retained bytes in the matched
private graph measurement, without dropping fields. Five page pairs preserve
canonical output exactly. The value type remains 32 bytes and `Send + Sync`;
the lazy borrowed-number cache is charged in instance cache accounting.

Measured OpenBridge cold physical peaks are 2.151/2.157 GB for cover/header,
down from 2.439/2.442 GB. Cold import still exceeds the 1 GB goal. Exact whole
scene compositing, remaining transform differences and the other operations
in the [active goal](engine-goal-scope.md) still require work.

Local immutable evidence directories: `optimized-engine-3`,
`engine-batch-3-focused`, `six-case-optimized-engine-3`,
`headless-contexts-optimized-3`, `multi-file-contexts-optimized-3`,
`node-query-lookup-after-3`, and `node-query-selection-before-2`.
Reports retain executable hashes, parameters and the underlying measurements.
