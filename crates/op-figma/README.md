# Comparing an imported Figma frame

The `render_fig_frame` example imports a local `.fig` file in `Preserve` mode,
selects a page and one top-level frame, and exports a PNG through the production
native renderer. It also enables `preserve_authored_geometry` on `EditorState`:
both switches are needed to retain authored positions through scene layout.

```sh
cargo run -p op-figma --example render_fig_frame -- \
  /path/to/document.fig PAGE_ID_OR_NAME FRAME_NAME /path/to/candidate.png \
  /path/to/exact-font.ttf /path/to/another-font.ttf
```

Pass the original font files required by the selected frame. Registering a
different revision of the same font can change both glyph outlines and spacing.
Fonts, design files, extracted images and reference-renderer binaries are local
inputs; they are not bundled into the example or this repository.

The example writes JSON containing the selected page/frame, painted bounds,
import warnings and separate prepare/convert/layout/render timings. The PNG uses
scale 1 and zero export margin. Optional `OP_DUMP_JSON` and `OP_DUMP_SCENE` paths
save import and scene diagnostics outside the repository.

Compare with a reference exported from the same unmodified node, at the same
pixel dimensions, scale, color profile and transparency. Fail a dimension
mismatch instead of resizing either render. Report pixel error and a perceptual
metric alongside an overlay or heatmap. Keep reference and candidate hashes in
the result so later runs cannot silently change the baseline.

The [PNG comparator](../../scripts/fig-render-parity/README.md) provides JSON
metrics, overlays and heatmaps to inspect each case.

Image similarity measures rendering differences for the selected frame. Feature
coverage needs separate cases for text styles, instance overrides, variables,
images, masks, effects and layout. A matching frame does not establish full
document parity. Debug timings from different cache states are observations,
not a performance comparison; measure release builds with repeated warm/cold
runs and record the machine and renderer revision for that purpose.

The parity branch includes semantic importer regressions and variable-font
tests. Run the focused checks with:

```sh
cargo test -p op-figma
cargo test -p op-host-native --features gl-host --lib backend::skia:: -- --test-threads=1
cargo clippy -p op-figma --all-targets -- -D warnings
```

Preserve mode honors authored reversed auto-layout paint stacking. Reflow mode
continues to emit flex flow order; independently representing reversed paint
order during reflow requires a separate scene capability.
