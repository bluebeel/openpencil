//! Gradient shader paint methods on [`NativeBackend`] plus their
//! pure helpers (`linear_gradient_endpoints`, `gradient_color_arrays`,
//! `premul_interpolation`, `fold_opacity`).
//!
//! Carved out of `skia.rs` so the parent file stays under the
//! workspace's 800-line cap. The methods continue to live on
//! `NativeBackend` via a sibling `impl` block; the helpers stay
//! `pub(super)` so the parent's tests module (`skia/tests.rs`) can
//! exercise them without re-exporting.

use super::{raster_point, to_sk_rect, NativeBackend};
use op_editor_ui::{Color, Rect};

#[path = "gradient_cache.rs"]
mod cache;
pub(super) use cache::{linear as linear_shader, radial as radial_shader};

impl NativeBackend {
    /// Fill a (round-)rectangle with a linear gradient. `stops`
    /// carries `(offset, color)` pairs ordered by ascending offset;
    /// `angle_deg` is the canonical `.op` angle (0° = bottom→top,
    /// 90° = left→right, matches CSS `to-top`); `opacity` is folded
    /// into each stop's alpha. Endpoints sit on the bounding ellipse,
    /// not the AABB — matches `pen-renderer/src/node-renderer.ts:155`
    /// so native, web, and export agree on gradient direction +
    /// extent.
    pub fn fill_round_rect_linear_gradient(
        &self,
        canvas: &skia_safe::Canvas,
        rect: Rect,
        radius: f32,
        stops: &[(f32, Color)],
        angle_deg: f32,
        opacity: f32,
    ) {
        if stops.is_empty() {
            return;
        }
        let (start, end) = linear_gradient_endpoints(self.active_paint_rect(rect), angle_deg);
        let Some(shader) = linear_shader(start, end, stops, opacity) else {
            // Skia refused to build the shader (degenerate input);
            // degrade to first-stop solid so the node still paints.
            self.fill_round_rect(canvas, rect, radius, fold_opacity(stops[0].1, opacity));
            return;
        };
        let mut paint = skia_safe::Paint::default();
        jian_skia::raster_sampling::configure_geometry(&mut paint);
        paint.set_shader(shader);
        raster_point::draw_round_rect(canvas, to_sk_rect(rect), radius, radius, &paint);
    }

    /// Fill a (round-)rectangle with a radial gradient. `cx_frac` /
    /// `cy_frac` are 0.0..=1.0 fractions of `rect`'s width / height
    /// for the centre; `radius_frac` is a 0.0..=1.0 fraction of
    /// `max(w, h)` for the outer radius — matches
    /// `pen-renderer/src/node-renderer.ts:187` so existing `.op`
    /// files render at the same radial size on native + web +
    /// export. Stops + opacity follow the linear convention.
    #[allow(clippy::too_many_arguments)]
    pub fn fill_round_rect_radial_gradient(
        &self,
        canvas: &skia_safe::Canvas,
        rect: Rect,
        radius: f32,
        stops: &[(f32, Color)],
        cx_frac: f32,
        cy_frac: f32,
        radius_frac: f32,
        opacity: f32,
    ) {
        if stops.is_empty() {
            return;
        }
        let paint_rect = self.active_paint_rect(rect);
        let center = skia_safe::Point::new(
            paint_rect.origin.x + paint_rect.size.x * cx_frac.clamp(0.0, 1.0),
            paint_rect.origin.y + paint_rect.size.y * cy_frac.clamp(0.0, 1.0),
        );
        // Match `pen-renderer/src/node-renderer.ts:187` — outer
        // radius is `radius_frac × max(w, h)`. Earlier this used
        // `min(w, h) / 2`, which made existing `.op` radial
        // gradients render at roughly 1/4 the size on native vs.
        // web/export.
        let outer = (paint_rect.size.x.max(paint_rect.size.y)) * radius_frac.clamp(0.0, 1.0);
        let outer = outer.max(0.01); // Sub-pixel radii confuse skia's shader.
        let Some(shader) = radial_shader(center, outer, stops, opacity) else {
            self.fill_round_rect(canvas, rect, radius, fold_opacity(stops[0].1, opacity));
            return;
        };
        let mut paint = skia_safe::Paint::default();
        jian_skia::raster_sampling::configure_geometry(&mut paint);
        paint.set_shader(shader);
        raster_point::draw_round_rect(canvas, to_sk_rect(rect), radius, radius, &paint);
    }

    /// Fill a (round-)rectangle with a Gouraud-interpolated mesh
    /// gradient. `colors` is a row-major `rows`×`cols` lattice
    /// (length == `rows * cols`); vertex `(r, c)` anchors the colour at
    /// rect position `(c/(cols-1), r/(rows-1))`. The grid is triangulated
    /// into `VertexMode::Triangles` (indexed when representable) and drawn via
    /// `Canvas::draw_vertices` under a round-rect clip so the fill
    /// respects the corner radius. `opacity` folds into the layer alpha.
    #[allow(clippy::too_many_arguments)]
    pub fn fill_round_rect_mesh_gradient(
        &self,
        canvas: &skia_safe::Canvas,
        rect: Rect,
        radius: f32,
        rows: u32,
        cols: u32,
        colors: &[Color],
        opacity: f32,
    ) {
        if self.has_paint_space() {
            super::raster_point::mesh_field(
                canvas,
                rect,
                self.active_paint_rect(rect),
                radius,
                rows,
                cols,
                colors,
                opacity,
                false,
            );
            return;
        }
        if super::raster_point::mesh(canvas, rect, radius, rows, cols, colors, opacity) {
            return;
        }
        let count = rows.max(2).checked_mul(cols.max(2)).map(|n| n as usize);
        if count != Some(colors.len()) {
            // Preserve the legacy malformed-grid solid fallback.
            if let Some(c) = colors.first() {
                self.fill_round_rect(canvas, rect, radius, fold_opacity(*c, opacity));
            }
            return;
        }
        let Some(vertices) = jian_skia::mesh_geometry::lattice_vertices(
            [rect.origin.x, rect.origin.y, rect.size.x, rect.size.y],
            rows,
            cols,
            colors.len(),
            |i| {
                let col = colors[i];
                let to_u8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                // The draw paint applies layer opacity exactly once, matching
                // the float-mesh path and Jian's mesh draw.
                skia_safe::Color::from_argb(to_u8(col.a), to_u8(col.r), to_u8(col.g), to_u8(col.b))
            },
        ) else {
            jian_skia::raster_sampling::mark_unsupported("unrepresentable mesh grid");
            return;
        };

        let restore = canvas.save();
        if radius > 0.5 {
            let rrect = skia_safe::RRect::new_rect_radii(
                to_sk_rect(rect),
                &[
                    skia_safe::Point::new(radius, radius),
                    skia_safe::Point::new(radius, radius),
                    skia_safe::Point::new(radius, radius),
                    skia_safe::Point::new(radius, radius),
                ],
            );
            canvas.clip_rrect(
                rrect,
                None,
                Some(jian_skia::raster_sampling::geometry_antialias()),
            );
        } else {
            canvas.clip_rect(
                to_sk_rect(rect),
                None,
                Some(jian_skia::raster_sampling::geometry_antialias()),
            );
        }
        // `draw_vertices` with no shader combines each interpolated vertex
        // colour with the paint's colour via `mode`. A default Paint is
        // opaque BLACK, so `Modulate` (vertex × black) would paint black;
        // seed the paint white so `Modulate` passes the vertex colours
        // through unchanged (white × vertex == vertex). `opacity` rides in
        // the paint's alpha.
        let mut paint = skia_safe::Paint::default();
        jian_skia::raster_sampling::configure_geometry(&mut paint);
        paint.set_color4f(
            skia_safe::Color4f::new(1.0, 1.0, 1.0, opacity.clamp(0.0, 1.0)),
            None,
        );
        canvas.draw_vertices(&vertices, skia_safe::BlendMode::Modulate, &paint);
        canvas.restore_to_count(restore);
    }

    /// Fill a (round-)rectangle with a native SkSL shader. `sksl` is the
    /// RAW (untrusted) source (entrypoint `half4 main(float2 fragCoord)`);
    /// `uniforms` carries `(name, values)` bindings (length 1 = float,
    /// 2/3/4 = vec*); `fallback` is the visible solid colour painted when
    /// the program fails to compile / build. The compiled `RuntimeEffect`
    /// is cached on `self.shader_cache` keyed by source hash, so the
    /// per-frame editor repaint reuses the program rather than
    /// recompiling. On ANY failure we paint `fallback` solid — never
    /// panic, since the source is untrusted.
    #[allow(clippy::too_many_arguments)]
    pub fn fill_round_rect_shader(
        &mut self,
        canvas: &skia_safe::Canvas,
        rect: Rect,
        radius: f32,
        sksl: &str,
        uniforms: &[(&str, &[f32])],
        opacity: f32,
        fallback: Color,
    ) {
        use skia_safe::runtime_effect::RuntimeShaderBuilder;

        let local_matrix = runtime_shader_local_matrix(self.active_paint_rect(rect), uniforms);
        let shader = self.shader_cache.get_or_compile(sksl).and_then(|effect| {
            // `RuntimeEffect` is an RCHandle — this is a refcount bump,
            // not a recompile.
            let mut builder = RuntimeShaderBuilder::new(effect);
            for (name, values) in uniforms {
                // Tolerate undeclared-name / arity mismatch so one bad
                // uniform doesn't sink the whole fill.
                let _ = builder.set_uniform_float(*name, values);
            }
            builder.make_shader(&local_matrix)
        });

        let mut paint = skia_safe::Paint::default();
        jian_skia::raster_sampling::configure_geometry(&mut paint);
        paint.set_alpha_f(opacity.clamp(0.0, 1.0));
        match shader {
            Some(s) => {
                paint.set_shader(s);
            }
            None => {
                // Compile / build failed — visible solid fallback.
                let c = fold_opacity(fallback, opacity);
                paint.set_color4f(
                    skia_safe::Color4f::new(
                        c.r.clamp(0.0, 1.0),
                        c.g.clamp(0.0, 1.0),
                        c.b.clamp(0.0, 1.0),
                        c.a.clamp(0.0, 1.0),
                    ),
                    None,
                );
                // Paint alpha is the Color4f alpha itself, not an independent
                // layer factor. Keep the authored fallback alpha set above.
            }
        }
        raster_point::draw_round_rect(canvas, to_sk_rect(rect), radius, radius, &paint);
    }

    /// Distinct SkSL programs compiled so far — exposed for the
    /// compile-cache proof (per-frame repaints must NOT grow this).
    pub fn shader_compile_count(&self) -> u64 {
        self.shader_cache.compile_count()
    }
}

/// Map RuntimeEffect coordinates back into the node's document-space
/// dimensions. Preset shaders receive an exact `float2 size` uniform;
/// arbitrary shaders without a valid size still get node-local coordinates.
fn runtime_shader_local_matrix(rect: Rect, uniforms: &[(&str, &[f32])]) -> skia_safe::Matrix {
    jian_skia::shader_geometry::local_matrix(
        [rect.origin.x, rect.origin.y, rect.size.x, rect.size.y],
        uniforms.iter().map(|(name, values)| (*name, *values)),
    )
}

/// Multiply a colour's alpha by a per-fill opacity factor. Used as
/// the fallback when skia refuses to build a gradient shader (e.g.
/// degenerate single-stop input).
pub(super) fn fold_opacity(c: Color, opacity: f32) -> Color {
    Color {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a * opacity.clamp(0.0, 1.0),
    }
}

/// Project the canonical `.op` `angle` onto the two gradient
/// endpoints, matching `pen-renderer/src/node-renderer.ts:155`
/// verbatim:
///
/// ```text
/// rad  = (angle - 90) · π/180     // 0° = bottom→top, 90° = left→right
/// x1   = cx - cos · w/2,   y1 = cy - sin · h/2
/// x2   = cx + cos · w/2,   y2 = cy + sin · h/2
/// ```
///
/// The endpoint scale uses `w/2` on the x-axis and `h/2` on the
/// y-axis — endpoints sit on the bounding ellipse, NOT the AABB.
/// Re-using the AABB-projection trick here would stretch the
/// gradient band at off-axis angles and diverge from the TS
/// renderer + the export pipeline.
pub(super) fn linear_gradient_endpoints(
    rect: Rect,
    angle_deg: f32,
) -> (skia_safe::Point, skia_safe::Point) {
    let cx = rect.origin.x + rect.size.x / 2.0;
    let cy = rect.origin.y + rect.size.y / 2.0;
    let rad = (angle_deg - 90.0).to_radians();
    let (sin, cos) = (rad.sin(), rad.cos());
    let dx = cos * rect.size.x * 0.5;
    let dy = sin * rect.size.y * 0.5;
    (
        skia_safe::Point::new(cx - dx, cy - dy),
        skia_safe::Point::new(cx + dx, cy + dy),
    )
}

/// Premultiplied-colour interpolation in the destination colour
/// space — matches what skia's deprecated `Flags::INTERPOLATE_COLORS_IN_PREMUL`
/// configured, with the new gradient API.
pub(super) fn premul_interpolation() -> skia_safe::gradient::Interpolation {
    use skia_safe::gradient::interpolation::{ColorSpace, HueMethod, InPremul};
    skia_safe::gradient::Interpolation {
        in_premul: InPremul::Yes,
        color_space: ColorSpace::Destination,
        hue_method: HueMethod::Shorter,
    }
}

/// Build the parallel `Color4f` + offset arrays skia's gradient
/// shaders consume from our `(offset, Color)` stops; `opacity` is
/// folded into each stop's alpha so the shader stays single-pass.
pub(super) fn gradient_color_arrays(
    stops: &[(f32, Color)],
    opacity: f32,
) -> (Vec<skia_safe::Color4f>, Vec<f32>) {
    let op = opacity.clamp(0.0, 1.0);
    let colors: Vec<skia_safe::Color4f> = stops
        .iter()
        .map(|(_, c)| {
            skia_safe::Color4f::new(
                c.r.clamp(0.0, 1.0),
                c.g.clamp(0.0, 1.0),
                c.b.clamp(0.0, 1.0),
                (c.a * op).clamp(0.0, 1.0),
            )
        })
        .collect();
    let offsets: Vec<f32> = stops.iter().map(|(t, _)| t.clamp(0.0, 1.0)).collect();
    (colors, offsets)
}
