# Native Figma node export

Build once, then run the binary directly for measured exports:

```sh
cargo build -j1 -p op-figma --example render_fig_frame
target/debug/examples/render_fig_frame input.fig page-id Card output.png font.ttf
```

The four positional arguments are input, page ID or unique page name, unique
top-level node name, and output PNG. Extra positional paths register local fonts.
The importer uses authored geometry, and the export uses the production native
renderer with zero export margin.

For descendants, add `--node-path '["Card",{"name":"Tile","index":1}]'`.
Each name must match one sibling unless an explicit zero-based index selects
among equal-name siblings. Missing names and ambiguous matches fail. Unnamed
canonical nodes match an empty string; whitespace names retain their spaces.
Alternatively, `--node-id canonical-id` matches exactly one converted node ID.
Generated IDs can change with importer revisions, so named paths are preferable
for maintained benchmark cases.

`--expect-bounds '[0,0,100,80]'` validates painted bounds within 0.02 document
pixels. `--expect-size 100x80` validates actual PNG dimensions without resizing.
`--authored-bounds` exports the selected scene node's own rectangle, preserving
transparent frame padding. Without it, export remains cropped to painted bounds.
`--scale` accepts 0.5 through 8, defaulting to 1. `--font path` also registers a
font explicitly. `--phase-events` emits stdout JSONL phase-start events for a
process-tree resource observer, including the importer's container, schema,
decode, style resolution, tree construction, source release and image resolution
phases. The final stdout line remains the result JSON.

Successful JSON includes resolved target identity/path, bounds, PNG dimensions,
font registrations, importer warnings, and read/font/preparation/conversion/
layout/render timings and import phase timestamps. Load is read plus preparation. Errors exit nonzero and
include a human-readable `error` plus typed `errorDetail` fields. Font-file
registration is recorded; per-style fallback events are not currently exposed.

Descendant exports use the production export API's coordinate behavior. Verify
their expected bounds rather than assuming ancestor transforms. Node selection
does not change importer internals or document schema.
