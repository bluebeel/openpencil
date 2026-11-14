# Engine performance and Figma fidelity goal

Status: active. The user explicitly expanded the existing “every engine
operation” goal to include every area below. This checklist is part of the
goal’s acceptance scope, not a statement that the features are complete.

Build a fully headless, agent-driven renderer/editor whose operations remain
fast with 10, 100 and 1,000 logical agents editing one or multiple open files.
Keep combined shared-service and renderer memory below 1 GB, use little CPU
when idle, and minimize CPU per operation. External AI model/agent processes
are outside that memory budget. Preserve exact `.fig` feature and rendering
fidelity; target SSIM 1.0 and exact RGBA identity, not rounded similarity.

## Explicit operation coverage

- Drawing and editing: create, move, resize, rotate, flip, duplicate, group,
  ungroup, reparent, reorder and bulk edits.
- Picking and interaction: hover/hit testing, overlapping/deep selection,
  marquee/multiselection, snapping, smart guides, measurements and rulers.
- Layout: nested auto-layout, hug/fill sizing, min/max constraints, grids,
  constraints, alignment/distribution and incremental reflow.
- Vector geometry: pen/path editing, boolean union/subtraction/intersection,
  winding rules, stroke outlines, caps, joins and dashes.
- Compositing: nested clipping, masks, blend modes, group opacity, gradients,
  shadows, layer/background blur and color/alpha correctness.
- Typography: editing, caret/range placement, wrapping, styled runs, lists,
  variable fonts, OpenType features, ligatures, RTL/bidi, emoji, CJK, fallback
  and exact font/resource readiness.
- Components and dependencies: nested instances, overrides, variants, swaps,
  detachment, variables/modes, styles and propagation to dependent nodes.
- Document queries: node IDs, tree traversal, search, bounds, spatial indexes
  and dependency lookup without repeated full-document copies or scans.
- View/render runtime: pan/zoom, partial redraw, visibility culling, incremental
  scene updates, GPU/raster asset work, cache reuse/eviction and bounded workers.
- Assets and interchange: image decode/crop/tile/fit, SVG import and vector
  serialization, PNG/JPEG/WebP/PDF export, clipboard and cross-file copy/paste.
- Persistence: autosave, durable batch/group commits, atomic saves, crash
  recovery, external-change detection and lazy page/asset loading.
- Collaboration: per-agent selection/clipboard/camera/tool state, atomic
  multi-operation transactions, cancellation, idempotent/safe retries,
  ordering/conflict handling, per-agent undo/redo and cross-file consistency.

## Required evidence

1. Inventory existing support and implement missing behavior required by the
   goal. Distinguish missing features from slow implementations.
2. Benchmark cold/warm operations in an optimized build; record median/tail
   latency, CPU, throughput, queue delay, RSS and physical footprint. Report
   logical client count separately from active request concurrency.
   The real-file matrix must include the complete OpenBridge and MUI documents
   at 1, 10, 100 and 1,000 agents. Record logical clients and active in-flight
   requests independently; a 32-request window does not prove 1,000 simultaneous
   operations. Inventory every operation family above and mark implemented,
   benchmarked, missing transport, unsupported, rejected or unmeasured cases.
   Report CPU/RAM and correctness per operation, including cold/warm behavior;
   synthetic workloads and selected-frame exports cannot replace this evidence.
3. Exercise single/multiple files, large/deep documents, sustained edits,
   resource pressure, cancellation and failure/recovery. Retain no-loss, ID,
   transaction, clipboard and persisted-state invariants.
4. Compare immutable native Figma references with authored bounds and exact
   fonts/images. Track feature coverage, raw RGB/alpha errors and SSIM.
5. Keep resource queues/caches/contexts bounded, maintain durability before
   acknowledging writes, and avoid document-wide work for small changes.

Current reports and entry points: [shared headless workloads](headless-agent-performance.md),
[agent context semantics](agent-contexts.md),
[shared-file benchmark](../../scripts/bench/headless-agents.py),
[operation matrix](../../scripts/bench/operation-matrix.py), and
[multiple-file benchmark](../../scripts/bench/multi-file-agents.py).

Initial priorities: agent isolation and multi-file authority; optimized
operation measurements; incremental layout/redraw and node indexing; durable
batch writes; remaining cold-import memory and exact Figma pixel differences.

## Iteration cadence

Batch several independent fixes across parallel agents before rebuilding the
shared optimized binaries. Use focused checks and small probes during the
batch. Run the full reference gate once when the combined batch is ready;
avoid repeating the complete test suites after each small change. Recheck
earlier results when a new failure, code change or unresolved concern warrants it.
