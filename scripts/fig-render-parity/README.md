# PNG comparison

Compare output from any renderer with an externally supplied reference PNG.
Inputs must have identical dimensions and be at least 11×11 pixels. Neither
image is resized. Design documents, fonts and reference-renderer binaries are
not part of this package.

From the repository root:

```sh
npm ci --prefix scripts/fig-render-parity
npm exec --prefix scripts/fig-render-parity -- playwright install chromium
node scripts/fig-render-parity/compare-png.mjs reference.png candidate.png output/comparison
npm test --prefix scripts/fig-render-parity
```

This writes `comparison.json`, `comparison-heatmap.png` and
`comparison-overlay.png`. The heatmap amplifies the largest RGBA difference
fourfold in red; the overlay averages premultiplied pixels with equal weights.

Metrics include raw RGB MAE/RMSE, alpha MAE, the fraction of pixels with a maximum
channel difference above 8, and SSIM on sRGB luma composited over white. SSIM uses
an 11×11 Gaussian window with sigma 1.5, population moments, and excludes a
5-pixel border. Input hashes, browser version and comparator hash are recorded.

SSIM is an image similarity measure, not a feature coverage percentage. Validate
fonts, dimensions, color profile, alpha, export bounds and source node identity
before interpreting a score. Use separate cases for different features, and
inspect the overlay or heatmap to understand a regression.

The native frame exporter and focused Rust checks are documented in
[`crates/op-figma/README.md`](../../crates/op-figma/README.md).
