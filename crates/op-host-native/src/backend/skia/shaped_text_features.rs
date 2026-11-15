//! Merge the existing shaping defaults with borrowed authored OpenType tags.
//! Same BTreeMap order/override rules as the owned merge, without key copies.

use skia_safe::textlayout::TextStyle;
use std::collections::BTreeMap;

pub(super) fn apply(
    style: &mut TextStyle,
    authored: Option<&BTreeMap<String, u32>>,
    kerning: Option<bool>,
) {
    let defaults = [
        ("calt", 0),
        ("kern", u32::from(kerning != Some(false))),
        ("liga", 0),
    ];
    let mut next = if kerning.is_some() { 0 } else { defaults.len() };
    let mut add = |tag: &str, value: u32| {
        if tag.len() == 4 && tag.is_ascii() {
            style.add_font_feature(tag, value as i32);
        }
    };
    for (tag, value) in authored.into_iter().flat_map(|features| features.iter()) {
        while next < defaults.len() && defaults[next].0 < tag.as_str() {
            add(defaults[next].0, defaults[next].1);
            next += 1;
        }
        if next < defaults.len() && defaults[next].0 == tag.as_str() {
            next += 1;
        }
        let value = if tag == "kern" && kerning == Some(false) {
            0
        } else {
            *value
        };
        add(tag, value);
    }
    for (tag, value) in &defaults[next..] {
        add(tag, *value);
    }
}
