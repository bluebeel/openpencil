# Batched engine checkpoint 4 — 2026-11-14

This batch combines bounded computed-layout queries, smaller Figma object keys,
ID allocation fast paths, authored-content preservation, image-fill rotation,
and opt-in correlated scene rendering. All source was frozen before the
combined optimized server/exporter build. Focused checks ran during the batch;
complete crate test suites were not repeated after each small fix.

## Behavior

Ordinary edits no longer enumerate every numeric node ID merely to set
metadata, geometry or styles. Pure leaf insertion avoids building the full
collision set while mixed batches and external allocators retain the legacy
collision checks. Figma documents in preserve-authored mode skip generated-UI
repair heuristics. Cleanup walks preserve absent children fields.

Computed-layout queries borrow the document, select the containing layout root,
and retain only bounded response data. They preserve sibling-dependent flex,
components, ancestor transforms and error/duplicate-ID behavior. Flat records
are nested in linear time.

Figma object keys use an eight-byte shared handle; values remain 32 bytes and
Send + Sync. Instance cache accounting includes key payload and unused backing
capacity. Five matched private page pairs preserve canonical imported output.
The retained graph shrank by 173,445,144 bytes; matched cold physical footprint
fell from 2.147 to 1.944 GB. This is still above the cold-import 1 GB target.
The one-shot cold wall time did not improve (12.087 to 12.299 seconds).

Authored IMAGE rotation, including explicit 360 degrees, now reaches native
sampling. The matrix uses the selected source dimensions and explicit float32
operations. Captured 90/360-degree matrices and WebGL probes match; the actual
Skia probe still differs on 16 Park pixels. Matrix identity alone is not a
claim of native pixel identity.

CLI exports can opt in with `--raster-sampling correlated4x2`; MCP `export_item`
uses `rasterSampling: "correlated4x2"`. The default remains Native. This mode
paints complete ordered scenes at four 2x camera positions, preserving overlap
before each byte resolve. It decodes/prepares once, reuses one source surface,
and retains original float colors for point-sampled geometry, meshes and text.
Its base scratch cap is 128 MiB (20*width*height bytes); nested effects, mesh data
and encoding are additional. It explicitly errors for unsupported bitmap/color
or missing outline glyphs, nearest tiled images, invalid rotated spans and
oversized working sets. Arbitrary FnOnce callbacks still execute once.

Focused checks include 30 native/export tests plus three Jian tests, full
333 importer tests before the cache-accounting follow-up (nine focused cache
checks after), and relevant allocator, layout/loader, host routing/persistence,
agent-context and authored-content checks. The correlated blend microprobes
match 1,024 measured solid and textured GPU cases, independently of the full
Figma reference comparisons below.

## Runtime evidence

Measured source: `06bc4bcc418f4142cad00dc7fd3f25588269f9c3`;
Jian: `13a4ed5a9641fb353c5fc16fb669f8ebb0a7cb32`.
The selectively optimized development build completed in 262.4 seconds with
clean, unchanged source. This is not a release-profile measurement.
Server SHA-256: `9f91136028be257537e4611be905925027ccc8b1934af020c537c5e235582045`.
Exporter SHA-256: `f371f46d27bf9fb41fe23943fb33c4041d3d8dcd166d3b3afdbf0b258017825f`.

One service, 1,000 logical agents, 32 active requests maximum, 2,000 mutation
requests and 8,000 drawings, with distinct agentId contexts:

| Workload | Batch 3 edit time | Batch 4 edit time | Batch 4 service CPU | Aggregate physical peak |
| --- | ---: | ---: | ---: | ---: |
| One shared file | 1.526 s | 0.639 s | 1.067 s | 287 MB |
| Three resident files | 5.214 s | 3.812 s | 8.130 s | 296 MB |

Service-only physical peaks were 104/113 MB. CPU covers the whole case;
aggregate memory includes the generator and excludes external models, sampled
at 50 ms. Request P95 was 17.4/161.6 ms. Counts, unique IDs, geometry and durable
saves passed, as did PNG and multi-file agent/document context checks. No
ambiguous writes were retried. Independent name-keyed comparison preserves all
8,000 authored positions, sizes and colors in each workload; all four benchmark
PNG files are byte-identical to batch 3. Durable file hashes differ because
cleanup no longer inserts absent empty-children arrays and concurrent ID/order
assignment can vary. No byte-identical document serialization is claimed.
These are workload measurements, not evidence for unlimited offered load or
allocator hard limits.

The 10k flat and nested operation matrices passed all 13 invariants, including
moves, resize, duplication, text, shadows, components, SVG/image pixels,
clipboard, zoom, undo/redo rendering, agent isolation and cross-file clipboard.
Their service CPU was 0.573/0.720 seconds and aggregate physical peaks were
384/626 MB. These serial checks do not establish per-agent undo isolation.

## Computed-layout queries

All 26 known normalized response hashes match exactly across the 13-case flat
and nested 10k-node fixtures. Flat small-parent depth-zero median fell from
45.17 to 0.903 ms; nested from 47.39 to 19.21 ms because the containing root
still includes sibling-dependent layout. Inactive-page queries fell from
42.30/46.13 ms to 0.516/0.684 ms. Flat empty-space query fell from 37.49 to
0.694 ms. Three wide/error baseline cases have one sample; other baseline
cases and current runs have three samples. These are small repeated samples,
not sustained-load tail guarantees.

Fresh bounded groups completed 11 non-wide queries on a flat 100k-node file,
with aggregate peak RSS <=854 MB and physical footprint <=768 MB; service-only
peaks were <=653 MB RSS and <=580 MB physical. The earlier 100k baseline had
failed its budget before replying, so no full 100k before/after speedup is
claimed. Initial completed small-parent replies match their known target
hashes.

Wide flat and several nested 100k operations still exceeded the stricter
aggregate 1 GB RSS guard, which counts the generator as well as the service.
The initial wide failure recorded 1,002,749,952 bytes aggregate RSS and
850,495,480 bytes physical. A fresh wide-depth-one group also failed. Persisted
service-only samples in later failing groups stayed below 1 GB (nested canvas
972 MB RSS/904 MB physical before termination), but those requests did not
finish. These failures neither prove service-only RSS exceeded 1 GB nor
certify a complete operation below the limit. The failure is retained in the
checkpoint; failed cases were not repeatedly rerun.

## Figma reference gate

All six correlated candidates rendered and compared with immutable references,
authored bounds and the exact fonts/images. Native was additionally checked
only for the two avatars: both PNG hashes are identical to batch 3. The full
Native six-case suite was not repeated. Nine live MCP smoke checks also passed:
default versus explicit Native byte identity, correlated opaque/overlap/alpha
PNG dimensions and representative pixels, invalid-mode errors, rejection of
correlated PDF, and valid default PDF. The first smoke fixture accidentally put
the background above the overlap; that private failed attempt was preserved,
the fixture ordering corrected, and only the small smoke was rerun.

| Case | Native batch 3 SSIM | Correlated SSIM | Correlated RGB MAE | Native differing pixels | Correlated differing pixels |
| --- | ---: | ---: | ---: | ---: | ---: |
| Park cover | 0.9962054089 | 0.9961374663 | 0.3676104 | 87,666 | 69,928 |
| Material UI welcome | 0.9974294577 | 0.9973866602 | 0.2208276 | 23,177 | 10,697 |
| OpenBridge cover | 0.9998478594 | 0.9998421495 | 0.0338604 | 6,446 | 3,573 |
| Park avatar | 0.9999944670 | 0.9999944670 | 0.0039876 | 16 | 16 |
| Material UI avatar | 1.0 | 1.0 | 0 | 0 | 0 |
| OpenBridge header | 0.9993980621 | 0.9993984563 | 0.0285884 | 1,371 | 1,030 |

The opt-in path reduces RGB MAE and differing pixels on four text-heavy frames,
but SSIM is slightly worse on the three covers. It is not a universal parity
improvement. OpenBridge header alpha MAE is 0.19824, cover alpha MAE 0.00001495;
other alpha MAEs are zero. Material UI avatar alone is exact RGBA. Park still
has 16 differing pixels. Exact whole-scene parity remains unfinished.

The opt-in mode is substantially slower on covers: Park 7,700 ms versus
396 ms Native, Material UI 2,257 versus 134 ms, OpenBridge 7,958 versus
3,404 ms. It remains an explicit comparison/development mode. Header render
was 78.5 ms; avatars 131.7/35.1 ms. Combined changes and cold effects are not
isolated causal benchmarks. Correlated cold OpenBridge physical peaks were
1.944/1.957 GB, still above the target.

No all-operation, unlimited-agent or cold-import-under-1-GB claim is made.
The broader [goal](engine-goal-scope.md) remains active. Next work should batch
wide/nested layout allocation reductions, remaining text/image UV/shadow
precision, correlated-mode throughput, and cold import memory. Per-agent
undo/redo, transaction/cancellation and the remaining engine operation coverage
remain in scope.


Immutable local evidence: `optimized-engine-4/build.json`,
`engine-batch-4-core/report.json`, `engine-batch-4-host/report.json`,
`correlated-renderer-focused-1/report.json`,
`headless-contexts-optimized-4/1000/report.json`,
`multi-file-contexts-optimized-4/1000/report.json`,
`contexts-optimized-4-comparison.json`, `operation-matrix-optimized-4-flat`,
`operation-matrix-optimized-4-nested`, `layout-query-optimized-4`,
`layout-query-optimized-4-nested10k`, `layout-query-optimized-4-groups`,
`six-case-correlated-engine-4/report.json`, `two-avatar-native-engine-4/report.json`,
`mcp-correlated-smoke-4/report.json`.
The benchmark scripts and reports retain exact parameters and executable hashes.
