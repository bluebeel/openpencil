//! Canonical text-content → payload conversion (styled-run aware).
//!
//! Split out of `adapter.rs` (800-line cap) when styled text landed.
//! `TextContent::Styled` keeps its flattened string for back-compat
//! (wrap, inline editing and hit-testing stay flat) and ADDITIONALLY
//! carries one [`TextRunPayload`] per segment so the canvas painter
//! can restyle each painted slice (per-segment font size / weight /
//! fill / italic / underline / strikethrough — TS rich-text parity).

// Two distinct `FontStyleKind` enums exist in the schema: the
// node-level one (`node::text`) and the per-segment one (`style`).
use jian_ops_schema::node::text::FontStyleKind as NodeFontStyle;
use jian_ops_schema::node::TextNode;
use jian_ops_schema::style::FontStyleKind as SegmentFontStyle;

use crate::payload::{NodePayload, TextRunPayload};
use crate::style_payload::parse_color;

/// Flatten `n.content` onto `p.text` and harvest styled segments into
/// `p.text_runs`. Also resolves the node-level italic / underline /
/// strikethrough flags; per-segment flags inherit the node level when
/// the segment doesn't override them (TS segment-style precedence).
pub(crate) fn apply_text_content(p: &mut NodePayload, n: &TextNode) {
    use jian_ops_schema::node::text::TextContent;
    p.italic = matches!(n.font_style, Some(NodeFontStyle::Italic));
    p.underline = n.underline.unwrap_or(false);
    p.strikethrough = n.strikethrough.unwrap_or(false);
    p.text_decoration_skip_ink = n.text_decoration_skip_ink;
    match &n.content {
        TextContent::Plain(s) => {
            p.text = Some(s.clone());
        }
        TextContent::Styled(segments) => {
            p.text = Some(segments.iter().map(|seg| seg.text.as_str()).collect());
            p.text_runs = segments
                .iter()
                .map(|seg| TextRunPayload {
                    text: seg.text.clone(),
                    font_size: (seg.font_size.unwrap_or(0.0) as f32).max(0.0),
                    font_weight: seg
                        .font_weight
                        .map(|w| w.min(u16::MAX as u32) as u16)
                        .unwrap_or(0),
                    fill: seg.fill.as_deref().and_then(parse_color),
                    italic: seg
                        .font_style
                        .as_ref()
                        .map(|s| matches!(s, SegmentFontStyle::Italic))
                        .unwrap_or(p.italic),
                    underline: seg.underline.unwrap_or(p.underline),
                    strikethrough: seg.strikethrough.unwrap_or(p.strikethrough),
                    text_decoration_skip_ink: seg
                        .text_decoration_skip_ink
                        .or(p.text_decoration_skip_ink),
                })
                .collect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decoration_skip_ink_inherits_node_and_preserves_segment_false() {
        let node: TextNode = serde_json::from_value(serde_json::json!({
            "id":"text", "textDecorationSkipInk":true,
            "content":[{"text":"A", "textDecorationSkipInk":false}, {"text":"é"}]
        }))
        .unwrap();
        let mut payload = crate::style_payload::base_payload(&node.base, "text");
        apply_text_content(&mut payload, &node);
        assert_eq!(payload.text.as_deref(), Some("Aé"));
        assert_eq!(payload.text_decoration_skip_ink, Some(true));
        assert_eq!(
            payload
                .text_runs
                .iter()
                .map(|r| r.text_decoration_skip_ink)
                .collect::<Vec<_>>(),
            vec![Some(false), Some(true)]
        );
        let legacy: TextNode =
            serde_json::from_value(serde_json::json!({"id":"old","content":"legacy"})).unwrap();
        apply_text_content(&mut payload, &legacy);
        assert_eq!(payload.text_decoration_skip_ink, None);
    }
}
