//! Typed SVG image elements and inherited single-image paint servers.
use super::*;
use base64::Engine;
use jian_ops_schema::{
    node::{ImageNode, ImageSrc},
    style::{
        ImageExtend, ImageFieldGeometry, ImageFieldUnits, ImageFillBody, ImageFillMode,
        ImagePatternGeometry, ImageViewportFit, ImageViewportGeometry,
    },
};
use kurbo::{Affine, Point};
pub(super) fn uri_bytes(uri: &str) -> Option<Vec<u8>> {
    let (header, body) = uri.strip_prefix("data:")?.split_once(',')?;
    if header.split(';').any(|p| p.eq_ignore_ascii_case("base64")) {
        base64::engine::general_purpose::STANDARD
            .decode(
                body.as_bytes()
                    .iter()
                    .copied()
                    .filter(|b| !b.is_ascii_whitespace())
                    .collect::<Vec<_>>(),
            )
            .ok()
    } else {
        let mut bytes = Vec::with_capacity(body.len());
        let b = body.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' {
                let h = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
                bytes.push(u8::from_str_radix(h, 16).ok()?);
                i += 3;
            } else {
                bytes.push(b[i]);
                i += 1;
            }
        }
        Some(bytes)
    }
}
fn source(resources: &SvgResources<'_>, uri: &str) -> (ImageSrc, Option<[u32; 2]>) {
    use std::hash::{Hash, Hasher};
    let mut state = std::collections::hash_map::DefaultHasher::new();
    uri.hash(&mut state);
    let hash = state.finish();
    if let Some(hit) = resources
        .images
        .borrow()
        .get(&hash)
        .and_then(|b| b.iter().find(|(s, _)| s.as_ref() == uri))
    {
        return hit.clone();
    }
    let dimensions =
        uri_bytes(uri).and_then(|b| crate::svg_image_composition::source_dimensions(&b));
    let result = (ImageSrc::from(uri), dimensions);
    resources
        .images
        .borrow_mut()
        .entry(hash)
        .or_default()
        .push(result.clone());
    result
}
fn href<'a>(el: &'a SvgTree, resources: &SvgResources<'a>) -> Option<&'a str> {
    let mut raw = resource_attr(el, "href").or_else(|| resource_attr(el, "xlink:href"))?;
    for _ in 0..64 {
        let Some(id) = raw.strip_prefix('#') else {
            return Some(raw);
        };
        let entry = resources.entries.get(id)?;
        if entry.node.tag != "image" {
            return None;
        }
        raw = resource_attr(entry.node, "href")
            .or_else(|| resource_attr(entry.node, "xlink:href"))?;
    }
    None
}
pub(super) fn dimension(raw: Option<&str>, span: f64) -> Option<f64> {
    let raw = raw?.trim();
    if raw == "auto" {
        return None;
    }
    if let Some(p) = raw.strip_suffix('%') {
        p.parse::<f64>().ok().map(|p| p * span / 100.0)
    } else {
        svg_number(raw)
    }
}
fn viewport_fit(raw: Option<&str>) -> (ImageViewportFit, [f64; 2]) {
    let mut tokens = raw.unwrap_or("xMidYMid meet").split_ascii_whitespace();
    let mut align = tokens.next().unwrap_or("xMidYMid");
    if align == "defer" {
        align = tokens.next().unwrap_or("xMidYMid");
    }
    if align == "none" {
        return (ImageViewportFit::None, [0.0, 0.0]);
    }
    let axis = |part: &str| match part {
        "Min" => 0.0,
        "Max" => 1.0,
        _ => 0.5,
    };
    let x = align.get(1..4).map(axis).unwrap_or(0.5);
    let y = align.get(5..8).map(axis).unwrap_or(0.5);
    (
        if tokens.next() == Some("slice") {
            ImageViewportFit::Slice
        } else {
            ImageViewportFit::Meet
        },
        [x, y],
    )
}
pub(super) fn box_of(matrix: Affine, w: f64, h: f64) -> [f64; 4] {
    let points = [
        Point::new(0.0, 0.0),
        Point::new(w, 0.0),
        Point::new(w, h),
        Point::new(0.0, h),
    ]
    .map(|p| matrix * p);
    let x = points.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
    let y = points.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
    let r = points.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
    let b = points.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);
    [x, y, r - x, b - y]
}
pub(super) fn image_element(
    el: &SvgTree,
    ctx: &StyleCtx,
    matrix: Affine,
    viewport: [f64; 4],
    resources: &SvgResources<'_>,
    allocator: &mut dyn crate::IdAllocator,
    taken: &mut std::collections::HashSet<NodeId>,
) -> Result<Option<PenNode>, IdAllocError> {
    let Some(uri) = href(el, resources) else {
        tracing::warn!("SVG image requires a media href");
        return Ok(None);
    };
    let (src, d) = source(resources, uri);
    let x = dimension(el.attr("x"), viewport[2]).unwrap_or(0.0);
    let y = dimension(el.attr("y"), viewport[3]).unwrap_or(0.0);
    let mut w = dimension(el.attr("width"), viewport[2]);
    let mut h = dimension(el.attr("height"), viewport[3]);
    let raw_size = [w.unwrap_or(0.0), h.unwrap_or(0.0)];
    if let Some([iw, ih]) = d {
        match (w, h) {
            (None, None) => {
                w = Some(iw as f64);
                h = Some(ih as f64);
            }
            (Some(v), None) => h = Some(v * ih as f64 / iw as f64),
            (None, Some(v)) => w = Some(v * iw as f64 / ih as f64),
            _ => {}
        }
    }
    if w.is_some_and(|v| v <= 0.0) || h.is_some_and(|v| v <= 0.0) {
        return Ok(None);
    }
    let placement = matrix * Affine::translate((x, y));
    let (fit, align) = viewport_fit(el.attr("preserveAspectRatio"));
    let clip = extract_style_or_attr(&el.attrs, "overflow").as_deref() != Some("visible");
    let id = allocator.allocate(taken)?;
    let mut image = ImageNode {
        src,
        nearest_sampling: ctx.image_rendering,
        ..Default::default()
    };
    image.base.id = id.into();
    image.base.name = el.attr("id").map(str::to_string);
    image.base.opacity = svg_node_opacity(&el.attrs);
    if let (Some(w), Some(h)) = (w, h) {
        let [x, y, bw, bh] = box_of(placement, w, h);
        if bw <= 0.0 || bh <= 0.0 {
            return Ok(None);
        }
        image.base.x = Some(x);
        image.base.y = Some(y);
        image.width = Some(SizingBehavior::Number(bw));
        image.height = Some(SizingBehavior::Number(bh));
        let normalized = Affine::scale_non_uniform(1.0 / bw, 1.0 / bh)
            * Affine::translate((-x, -y))
            * placement
            * Affine::scale_non_uniform(w, h);
        image.geometry = Some(Box::new(ImageFieldGeometry {
            transform: normalized.as_coeffs(),
            viewport: Some(Box::new(ImageViewportGeometry {
                size: [w, h],
                fit,
                align,
                clip,
            })),
            ..Default::default()
        }));
        if !clip {
            if let Some(dimensions) = d {
                let old = [x, y, bw, bh];
                if let Some(g) = image.geometry.as_ref() {
                    if let Ok(setup) =
                        crate::image_field_geometry::prepare_with_source(g, old, dimensions)
                    {
                        if let Some([sx, sy, sw, sh]) =
                            crate::image_field_geometry::source_bounds(&setup)
                        {
                            let expanded = [
                                x.min(sx),
                                y.min(sy),
                                (x + bw).max(sx + sw) - x.min(sx),
                                (y + bh).max(sy + sh) - y.min(sy),
                            ];
                            if let Ok(rebased) = crate::image_field_geometry::rebase(
                                g,
                                old,
                                Affine::IDENTITY,
                                expanded,
                            ) {
                                image.base.x = Some(expanded[0]);
                                image.base.y = Some(expanded[1]);
                                image.width = Some(SizingBehavior::Number(expanded[2]));
                                image.height = Some(SizingBehavior::Number(expanded[3]));
                                image.geometry = Some(Box::new(rebased));
                            }
                        }
                    }
                }
            }
        }
    } else {
        // The source and authored matrix survive a pending resource. No fake
        // natural dimensions are used to certify rendering readiness.
        let [a, b, c, d, e, f] = placement.as_coeffs();
        image.base.x = Some(e);
        image.base.y = Some(f);
        image.width = w.map(SizingBehavior::Number);
        image.height = h.map(SizingBehavior::Number);
        image.geometry = Some(Box::new(ImageFieldGeometry {
            units: ImageFieldUnits::LocalPixels,
            transform: [a, b, c, d, 0.0, 0.0],
            viewport: Some(Box::new(ImageViewportGeometry {
                size: raw_size,
                fit,
                align,
                clip,
            })),
            ..Default::default()
        }));
        tracing::warn!(source=%uri,"SVG image intrinsic layout is pending media resolution");
    }
    Ok(Some(PenNode::Image(image)))
}
pub(super) struct Pattern<'r, 'a> {
    pub chain: Vec<&'r Definition<'a>>,
}
impl Pattern<'_, '_> {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.chain.iter().find_map(|e| resource_attr(e.node, name))
    }
}
pub(super) fn pattern_chain<'r, 'a>(
    resources: &'r SvgResources<'a>,
    id: &str,
) -> Result<Pattern<'r, 'a>, SvgResourceError> {
    let mut seen = std::collections::HashSet::new();
    let mut chain = Vec::new();
    let mut next = Some(id);
    while let Some(id) = next {
        if chain.len() >= 64 {
            return Err(SvgResourceError::DepthLimit);
        }
        if !seen.insert(id.to_owned()) {
            return Err(SvgResourceError::Cycle { id: id.into() });
        }
        let entry = resources
            .entries
            .get(id)
            .ok_or_else(|| SvgResourceError::Missing { id: id.into() })?;
        if entry.node.tag != "pattern" {
            return Err(SvgResourceError::Invalid {
                attribute: "image pattern href",
            });
        }
        chain.push(entry);
        next = resource_href(entry.node);
    }
    Ok(Pattern { chain })
}
pub(super) fn image_pattern(
    resources: &SvgResources<'_>,
    id: &str,
    opacity: f64,
    space: PaintSpace,
) -> Result<Option<PenFill>, SvgResourceError> {
    let entry = resources
        .entries
        .get(id)
        .ok_or_else(|| SvgResourceError::Missing { id: id.into() })?;
    if resource_attr(entry.node, crate::svg_image_composition::METADATA_ATTRIBUTE).is_some() {
        return restore_image(resources, entry, opacity, space);
    }
    let pattern = pattern_chain(resources, id)?;
    let Some(content) = pattern
        .chain
        .iter()
        .find(|e| e.node.children.iter().any(|n| !is_definition(&n.tag)))
    else {
        return Ok(None);
    };
    let inherited = &pattern.chain[0].context;
    let Some(image_content) = single_image(
        &content.node.children,
        inherited,
        Affine::IDENTITY,
        1.0,
        resources,
    )?
    else {
        return Ok(None);
    };
    let image = image_content.node;
    let Some(uri) = href(image, resources) else {
        return Ok(None);
    };
    let (src, source_dimensions) = source(resources, uri);
    let object = pattern.attr("patternUnits").unwrap_or("objectBoundingBox") == "objectBoundingBox";
    let b = space.source;
    if object && (b.w <= 0.0 || b.h <= 0.0) {
        return Ok(None);
    }
    let unit = if object {
        Affine::translate((b.x, b.y)) * Affine::scale_non_uniform(b.w, b.h)
    } else {
        Affine::IDENTITY
    };
    let span = if object {
        [1.0, 1.0]
    } else {
        [space.viewport[2], space.viewport[3]]
    };
    let x = dimension(pattern.attr("x"), span[0]).unwrap_or(0.0);
    let y = dimension(pattern.attr("y"), span[1]).unwrap_or(0.0);
    let w = dimension(pattern.attr("width"), span[0]).unwrap_or(0.0);
    let h = dimension(pattern.attr("height"), span[1]).unwrap_or(0.0);
    if w <= 0.0 || h <= 0.0 {
        return Ok(None);
    }
    let pt = pattern
        .attr("patternTransform")
        .map(|v| transform_list(&[("transform".into(), v.into())]))
        .transpose()
        .map_err(|_| SvgResourceError::Invalid {
            attribute: "patternTransform",
        })?
        .unwrap_or(Affine::IDENTITY);
    let tile_source = unit * pt * Affine::translate((x, y)) * Affine::scale_non_uniform(w, h);
    let target = Affine::scale_non_uniform(1.0 / space.target.w, 1.0 / space.target.h)
        * Affine::translate((-space.target.x, -space.target.y))
        * space.matrix;
    let tile = target * tile_source;
    let content_matrix = if let Some(raw) = pattern.attr("viewBox") {
        let attrs = vec![
            ("viewBox".into(), raw.into()),
            (
                "preserveAspectRatio".into(),
                pattern
                    .attr("preserveAspectRatio")
                    .unwrap_or("xMidYMid meet")
                    .into(),
            ),
        ];
        Affine::scale_non_uniform(1.0 / w, 1.0 / h)
            * super::transform::viewbox_matrix(
                &attrs,
                super::transform::viewbox(&attrs).map_err(|_| SvgResourceError::Invalid {
                    attribute: "pattern viewBox",
                })?,
                w,
                h,
            )
            .map_err(|_| SvgResourceError::Invalid {
                attribute: "pattern viewport",
            })?
    } else {
        // With no viewBox, pattern content has the tile's origin. Only the
        // units' linear scale changes: x/y position the tile, not its children.
        let content = if pattern.attr("patternContentUnits") == Some("objectBoundingBox") {
            Affine::scale_non_uniform(b.w, b.h)
        } else {
            Affine::IDENTITY
        };
        let units = if object {
            Affine::scale_non_uniform(b.w, b.h)
        } else {
            Affine::IDENTITY
        };
        Affine::scale_non_uniform(1.0 / w, 1.0 / h) * units.inverse() * content
    };
    let content_box = box_of(content_matrix.inverse(), 1.0, 1.0);
    let content_span = [content_box[2], content_box[3]];
    let iw = dimension(image.attr("width"), content_span[0]).unwrap_or(0.0);
    let ih = dimension(image.attr("height"), content_span[1]).unwrap_or(0.0);
    if iw <= 0.0 || ih <= 0.0 {
        return Ok(None);
    }
    let ix = dimension(image.attr("x"), content_span[0]).unwrap_or(0.0);
    let iy = dimension(image.attr("y"), content_span[1]).unwrap_or(0.0);
    let transform = content_matrix
        * image_content.transform
        * Affine::translate((ix, iy))
        * Affine::scale_non_uniform(iw, ih);
    let (fit, align) = viewport_fit(image.attr("preserveAspectRatio"));
    let geometry = ImageFieldGeometry {
        transform: transform.as_coeffs(),
        viewport: Some(Box::new(ImageViewportGeometry {
            size: [iw, ih],
            fit,
            align,
            clip: extract_style_or_attr(&image.attrs, "overflow").as_deref() != Some("visible"),
        })),
        pattern: Some(Box::new(ImagePatternGeometry {
            transform: tile.as_coeffs(),
            clip: extract_style_or_attr(&pattern.chain[0].node.attrs, "overflow").as_deref()
                != Some("visible"),
            ..Default::default()
        })),
        extend_x: ImageExtend::Decal,
        extend_y: ImageExtend::Decal,
        ..Default::default()
    };
    Ok(Some(PenFill::Image(Box::new(ImageFillBody {
        url: src,
        geometry: Some(Box::new(geometry)),
        mode: Some(ImageFillMode::Stretch),
        original_size: source_dimensions.map(|d| jian_ops_schema::style::ImageOriginalSize {
            width: d[0] as f64,
            height: d[1] as f64,
        }),
        opacity: Some(opacity * image_content.opacity),
        nearest_sampling: image_content.context.image_rendering,
        ..Default::default()
    }))))
}
