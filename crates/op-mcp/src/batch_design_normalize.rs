//! Lenient JSON node-shape normalization for `batch_design`: rename passes
//! for the Figma / Pencil dialects, keyword canonicalization for layout /
//! sizing / text fields, stroke + padding repair, and the temporary-id
//! stamper.
//!
//! Split out of `batch_design.rs` to stay under the 800-line cap.

#[path = "batch_design_fill_normalize.rs"]
mod fill_normalize;
use fill_normalize::{normalize_fill, solid_fill};

pub(crate) fn normalize_node_shape(value: &mut serde_json::Value) {
    // Canonicalize authoring-only video nodes before any shape normalizer or
    // typed PenNode parse can inspect their discriminator.
    op_pen_loader::normalize_video_alias(value);
    normalize_node_shape_inner(value, true);
}

/// A replacement already has authored sizing semantics. Normalize dialects
/// without materializing omitted text bounds as generation defaults.
pub(crate) fn normalize_replacement_node_shape(value: &mut serde_json::Value) {
    op_pen_loader::normalize_video_alias(value);
    normalize_node_shape_inner(value, false);
}

fn normalize_node_shape_inner(value: &mut serde_json::Value, add_text_defaults: bool) {
    let serde_json::Value::Object(obj) = value else {
        return;
    };
    // Map Figma/Pencil auto-layout field names onto our schema FIRST — MiniMax-M3
    // (trained on Pencil's schema) emits `layoutMode`/`itemSpacing`/`strokeWeight`/
    // `primaryAxisAlignItems`/`counterAxisAlignItems`, which serde would silently
    // drop as unknown keys, leaving the frame with no layout → it renders as an
    // unstyled horizontal strip. Rename before anything else reads them.
    normalize_pencil_autolayout_dialect(obj);
    // Flatten a STRUCTURED `layout` object (`{type,gap,padding}` or the
    // externally-tagged `{Horizontal:{…}}`) down to our flat `layout` string +
    // hoisted gap/padding. glm-5.2 in the loop emits this Figma/flex shape; serde
    // rejects it against the string-typed `layout` field, so the WHOLE update
    // fails and the root never gets its layout (measured: glm built n1/n2/n3
    // correctly, then every `U(n1,{layout:{type:horizontal…}})` was rejected and
    // the tree thrashed to empty).
    normalize_layout_object(obj);
    if let Some(fill) = obj.get_mut("fill") {
        normalize_fill(fill);
    }
    // A weak model sometimes emits an empty stroke (`"stroke":[]` or `""`),
    // which deserializes as a 0-length `PenStroke` and fails the WHOLE node
    // (and cascades to every child that targets its binding). Treat an empty
    // stroke as "no stroke" — drop the key so the node still lands.
    if obj.get("stroke").is_some_and(|s| {
        matches!(s, serde_json::Value::Array(a) if a.is_empty())
            || matches!(s, serde_json::Value::String(t) if t.trim().is_empty())
            || s.is_null()
    }) {
        obj.remove("stroke");
    }
    if let Some(stroke) = obj.get_mut("stroke") {
        normalize_stroke(stroke);
    }
    if let Some(padding) = obj.get_mut("padding") {
        normalize_padding(padding);
    }
    // `effects` bodies carry REQUIRED fields (`spread` and friends). A model
    // that omits one fails the whole node, and in the program DSL that
    // cascades through every descendant line — see `effect_normalize`.
    crate::effect_normalize::normalize_node_effects(obj);
    if add_text_defaults {
        super::node_shape_defaults::normalize_text_default_bounds(obj);
    }
    normalize_layout_keyword(obj, "justifyContent");
    normalize_layout_keyword(obj, "alignItems");
    normalize_image_src(obj);
    normalize_text_growth(obj);
    normalize_text_align(obj);
    normalize_text_content(obj);
    normalize_sizing_keyword(obj, "width");
    normalize_sizing_keyword(obj, "height");
    if let Some(serde_json::Value::Array(children)) = obj.get_mut("children") {
        for child in children {
            normalize_node_shape_inner(child, add_text_defaults);
        }
    }
}

/// Rename Figma / Pencil auto-layout keys onto OpenPencil's schema so a model
/// trained on the Pencil dialect (MiniMax-M3) keeps its layout. Each rename is
/// applied ONLY when the canonical key is absent, so a node that already uses
/// our names is untouched. Axis-align values are lifted from Figma's
/// `MIN/MAX/CENTER/SPACE_BETWEEN` enum; unknown spellings pass through for the
/// downstream `normalize_layout_keyword` pass to handle.
fn normalize_pencil_autolayout_dialect(obj: &mut serde_json::Map<String, serde_json::Value>) {
    // layoutMode / direction → layout. `direction` is the flex/CSS alias glm-5.2
    // reaches for (measured: `{…,"direction":"horizontal",…}`), `layoutMode` the
    // Figma one M3 uses. First alias present wins.
    if !obj.contains_key("layout") {
        for alias in ["layoutMode", "direction"] {
            let Some(serde_json::Value::String(s)) = obj.remove(alias) else {
                continue;
            };
            let mapped = match s.trim().to_ascii_lowercase().as_str() {
                "horizontal" | "row" => Some("horizontal"),
                "vertical" | "column" => Some("vertical"),
                "none" | "" => Some("none"),
                _ => None,
            };
            if let Some(m) = mapped {
                obj.insert("layout".into(), serde_json::Value::String(m.into()));
            }
            break;
        }
    }
    // itemSpacing → gap
    if !obj.contains_key("gap") {
        if let Some(v) = obj
            .remove("itemSpacing")
            .filter(serde_json::Value::is_number)
        {
            obj.insert("gap".into(), v);
        }
    }
    // primaryAxisAlignItems → justifyContent ; counterAxisAlignItems → alignItems
    for (from, to) in [
        ("primaryAxisAlignItems", "justifyContent"),
        ("counterAxisAlignItems", "alignItems"),
    ] {
        if obj.contains_key(to) {
            continue;
        }
        let Some(serde_json::Value::String(s)) = obj.remove(from) else {
            continue;
        };
        let raw = s.trim();
        let mapped = [
            ("MIN", "start"),
            ("MAX", "end"),
            ("CENTER", "center"),
            ("SPACE_BETWEEN", "space_between"),
        ]
        .into_iter()
        .find_map(|(alias, value)| raw.eq_ignore_ascii_case(alias).then_some(value));
        obj.insert(
            to.into(),
            serde_json::Value::String(mapped.map_or(s, str::to_owned)),
        );
    }
    // strokeWeight → stroke thickness (Figma names the width apart from the color)
    if let Some(weight) = obj
        .remove("strokeWeight")
        .filter(serde_json::Value::is_number)
    {
        match obj.get_mut("stroke") {
            Some(serde_json::Value::Object(s)) => {
                s.entry("thickness").or_insert(weight);
            }
            Some(serde_json::Value::String(color)) => {
                let color = std::mem::take(color);
                obj.insert(
                    "stroke".into(),
                    serde_json::json!({ "thickness": weight, "fill": color }),
                );
            }
            _ => {}
        }
    }
}

/// Flatten a STRUCTURED `layout` object onto our flat schema. glm-5.2 in the
/// agentic loop writes `layout` as a Figma/flex object —
/// `{"type":"horizontal","gap":0,"padding":[…]}` or the externally-tagged
/// `{"Horizontal":{"gap":0,…}}` — but our `layout` field is a plain string
/// (`"horizontal"`), with `gap`/`padding`/`justifyContent`/`alignItems` as
/// sibling keys. serde rejects the object, failing the whole insert/update, so
/// the node's layout never lands (measured: glm built its tree with correct ids,
/// then every `U(n1,{layout:{type:horizontal…}})` was rejected). Lift the
/// direction to the `layout` string and hoist the object's spacing keys to the
/// node (only where the node doesn't already set them). Unknown directions are
/// left untouched rather than guessed.
fn normalize_layout_object(obj: &mut serde_json::Map<String, serde_json::Value>) {
    use serde_json::Value;
    let Some(Value::Object(layout)) = obj.get("layout") else {
        return;
    };
    let (raw, variant) = if let Some(raw) = layout.get("type").and_then(Value::as_str) {
        (raw, false)
    } else if let Some((key, value)) = layout.iter().next() {
        (key.as_str(), value.is_object())
    } else {
        return;
    };
    let Some(direction) = layout_direction(raw) else {
        return;
    };
    // Replacing the existing slot preserves preserve_order map position while
    // taking ownership of the structured spacing data instead of cloning it.
    let Some(Value::Object(layout)) = obj
        .get_mut("layout")
        .map(|value| std::mem::replace(value, Value::String(direction.into())))
    else {
        return;
    };
    let mut source = if variant {
        match layout.into_iter().next() {
            Some((_, Value::Object(inner))) => inner,
            _ => return,
        }
    } else {
        layout
    };
    for key in ["gap", "padding", "justifyContent", "alignItems"] {
        if let Some(value) = source.remove(key) {
            obj.entry(key).or_insert(value);
        }
    }
}

fn layout_direction(raw: &str) -> Option<&'static str> {
    let raw = raw.trim();
    [
        ("horizontal", "horizontal"),
        ("row", "horizontal"),
        ("vertical", "vertical"),
        ("column", "vertical"),
        ("none", "none"),
    ]
    .into_iter()
    .find_map(|(alias, direction)| raw.eq_ignore_ascii_case(alias).then_some(direction))
}

/// `width` / `height` accept `fill_container` / `fit_content` / a number. A weak
/// model sometimes appends a type-hint suffix (`fill_container_str` — a leaked
/// variable name) or uses a CSS-ish spelling (`fill-container` / `hug`), which
/// fails the `Sizing` enum and drops the whole node. Map the known content-hug /
/// fill spellings back to the canonical keyword; DROP the ambiguous `auto`
/// (schema default wins); leave numbers, numeric strings, and already-valid /
/// unrecognised values untouched.
fn normalize_sizing_keyword(obj: &mut serde_json::Map<String, serde_json::Value>, key: &str) {
    let Some(serde_json::Value::String(raw)) = obj.get(key) else {
        return;
    };
    if matches!(raw.as_str(), "fill_container" | "fit_content")
        || !raw.bytes().any(|byte| byte.is_ascii_alphabetic())
    {
        return;
    }
    let mut canon = raw.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    for suffix in ["_str", "_string", "_val", "_value"] {
        if let Some(stripped) = canon.strip_suffix(suffix) {
            canon = stripped.to_string();
            break;
        }
    }
    let normalized = match canon.as_str() {
        "fill_container" | "fillcontainer" | "fill" | "container" | "full" | "fill_width"
        | "fill_parent" => Some("fill_container"),
        "fit_content" | "fitcontent" | "fit" | "hug" | "hug_content" | "content" => {
            Some("fit_content")
        }
        // CSS `auto` is AMBIGUOUS: for a block's width it usually means
        // stretch/fill, for height it means hug — forcing either direction
        // inverts the author's intent half the time. Drop the key so the node
        // survives deserialization with the schema default instead.
        "auto" => {
            obj.remove(key);
            return;
        }
        _ => None,
    };
    if let Some(valid) = normalized {
        obj.insert(key.into(), serde_json::Value::String(valid.to_string()));
    }
}

/// A weak model sometimes emits an `image` node with NO `src` (or puts the URL
/// under an alias like `url`/`source`). `ImageNode.src` is REQUIRED, so the whole
/// node fails to deserialize and is dropped — the avatar/logo vanishes AND the
/// column it anchored collapses. Recover the src from a common alias, else inject
/// an empty placeholder (renders as a grey box) so the node — and the layout it
/// holds open — survives. (Measured: glm avatar images → 7× `missing field src`.)
fn normalize_image_src(obj: &mut serde_json::Map<String, serde_json::Value>) {
    if obj.get("type").and_then(serde_json::Value::as_str) != Some("image") {
        return;
    }
    let has_src = obj
        .get("src")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|t| !t.trim().is_empty());
    if has_src {
        return;
    }
    for alias in ["url", "source", "imageUrl", "image_url", "uri", "href"] {
        if obj
            .get(alias)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|text| !text.trim().is_empty())
        {
            if let Some(value) = obj.get(alias).cloned() {
                obj.insert("src".into(), value);
            }
            return;
        }
    }
    obj.insert("src".into(), serde_json::Value::String(String::new()));
}

/// `textGrowth` only accepts `auto` / `fixed-width` / `fixed-width-height`. A
/// weak model borrows a SIZING keyword (`fit_content` / `fill_container`) for it,
/// and the invalid variant drops the whole text node. Map the content-hugging
/// forms to `auto` (same intent); drop anything else so the node still lands with
/// the default growth. (Measured: glm text nodes → 3× `unknown variant fit_content`.)
fn normalize_text_growth(obj: &mut serde_json::Map<String, serde_json::Value>) {
    let Some(raw) = obj.get("textGrowth").and_then(serde_json::Value::as_str) else {
        return;
    };
    if matches!(raw, "auto" | "fixed-width" | "fixed-width-height") {
        return;
    }
    let canon = raw.trim().to_ascii_lowercase().replace([' ', '_'], "-");
    let normalized = match canon.as_str() {
        "auto" | "fit-content" | "hug" | "fill-container" | "fill" | "fit" => Some("auto"),
        "fixed-width" => Some("fixed-width"),
        "fixed-width-height" | "fixed-width-and-height" | "fixed" => Some("fixed-width-height"),
        _ => None,
    };
    // Unrecognized spellings: recover the intent from the words before falling
    // back to removal ("fixed_width_and_height", "fixedWidth" and friends carry
    // a clear meaning — silently reverting them to the default undoes a
    // deliberate wrap request).
    let normalized = normalized.or_else(|| {
        if canon.contains("width") && canon.contains("height") {
            Some("fixed-width-height")
        } else if canon.contains("width") {
            Some("fixed-width")
        } else {
            None
        }
    });
    match normalized {
        Some(valid) => {
            obj.insert(
                "textGrowth".into(),
                serde_json::Value::String(valid.to_string()),
            );
        }
        None => {
            obj.remove("textGrowth");
        }
    }
}

/// `textAlign` is a physical four-value enum, unlike the layout-axis alignment
/// fields above. Models borrowing CSS logical alignment sometimes emit
/// `start` / `end`; translate those values before serde sees them so the text
/// node survives. Keep this scoped to the exact text property — container
/// `justifyContent` / `alignItems` legitimately use `start` / `end`.
fn normalize_text_align(obj: &mut serde_json::Map<String, serde_json::Value>) {
    let Some(serde_json::Value::String(value)) = obj.get_mut("textAlign") else {
        return;
    };
    let raw = value.trim();
    let normalized = if raw.eq_ignore_ascii_case("start") {
        "left"
    } else if raw.eq_ignore_ascii_case("end") {
        "right"
    } else {
        return;
    };
    value.clear();
    value.push_str(normalized);
}

/// Text node `content` must be a string or a styled-text-segment array. A weak
/// model occasionally writes a JSON number (`2024`, `8.5`) as the content, which
/// deserializes as neither and fails the whole text node. Coerce numbers to their
/// string form (`2024` → `"2024"`, `8.5` → `"8.5"`). Booleans and null are left
/// untouched so the schema can reject them properly.
fn normalize_text_content(obj: &mut serde_json::Map<String, serde_json::Value>) {
    if obj.get("type").and_then(serde_json::Value::as_str) != Some("text") {
        return;
    }
    let Some(content) = obj.get_mut("content") else {
        return;
    };
    if let serde_json::Value::Number(num) = content {
        *content = serde_json::Value::String(num.to_string());
    }
}

#[cfg(test)]
#[path = "batch_design_normalize_tests.rs"]
mod tests;

fn normalize_layout_keyword(obj: &mut serde_json::Map<String, serde_json::Value>, key: &str) {
    let Some(serde_json::Value::String(value)) = obj.get_mut(key) else {
        return;
    };
    let normalized = match (key, value.as_str()) {
        // CSS flexbox value names. A model fluent in CSS (glm-5.2 etc.) writes
        // `flex-start`/`flex-end` AND — by analogy to our snake_case `space_between`
        // — the underscore form `flex_start`/`flex_end`. The schema only accepts
        // `start`/`end`, so without this the WHOLE node fails to deserialize and is
        // silently dropped (a 5-column table loses every cell whose alignment is a
        // flex_* name — measured: glm dropped the right-aligned amount column + the
        // left-aligned header labels, keeping only the `center` ones).
        ("justifyContent" | "alignItems", "flex-start" | "flex_start" | "flexstart") => "start",
        ("justifyContent" | "alignItems", "flex-end" | "flex_end" | "flexend") => "end",
        ("justifyContent", "space-between") => "space_between",
        ("justifyContent", "space-around") => "space_around",
        ("justifyContent", "space-evenly" | "space_evenly") => "space_between",
        _ => return,
    };
    *value = normalized.to_string();
}

fn normalize_padding(value: &mut serde_json::Value) {
    let serde_json::Value::Array(items) = value else {
        return;
    };
    match items.len() {
        1 => {
            *value = items.pop().expect("one padding item");
        }
        3 => {
            items.push(items[1].clone());
        }
        _ => {}
    }
}

fn normalize_stroke(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(color) => {
            let fill = serde_json::Value::Array(vec![solid_fill(serde_json::Value::String(
                std::mem::take(color),
            ))]);
            *value = serde_json::Value::Object(serde_json::Map::from_iter([
                ("thickness".into(), serde_json::json!(1)),
                ("fill".into(), fill),
            ]));
        }
        serde_json::Value::Object(obj) => {
            if let Some(color) = obj.remove("color") {
                obj.entry("fill")
                    .or_insert_with(|| serde_json::Value::Array(vec![solid_fill(color)]));
            }
            if let Some(fill) = obj.get_mut("fill") {
                normalize_fill(fill);
            }
            // `thickness` is REQUIRED by the schema, but models write
            // `{color}` / `{width}` / `{weight}` stroke objects without it —
            // and one rejected node cascades into "parent not found" for its
            // whole subtree (measured: a DeepSeek V4 run dropped 60+ lines
            // and shipped one empty section, 2026-07-12). Alias the common
            // spellings, then default to a hairline.
            if !obj.contains_key("thickness") {
                let aliased = ["width", "weight", "strokeWeight"]
                    .iter()
                    .find_map(|alias| obj.remove(*alias));
                obj.insert(
                    "thickness".into(),
                    aliased.unwrap_or_else(|| serde_json::json!(1)),
                );
            }
        }
        _ => {}
    }
}

pub(crate) fn ensure_node_ids(value: &mut serde_json::Value, next: &mut usize) {
    let serde_json::Value::Object(obj) = value else {
        return;
    };
    if obj.contains_key("type") && !obj.contains_key("id") {
        obj.insert(
            "id".into(),
            serde_json::Value::String(format!("__op_tmp_{next}")),
        );
        *next += 1;
    }
    if let Some(serde_json::Value::Array(children)) = obj.get_mut("children") {
        for child in children {
            ensure_node_ids(child, next);
        }
    }
}
