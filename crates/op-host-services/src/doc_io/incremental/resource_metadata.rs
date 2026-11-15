//! Keep exact unchanged resource packets at their original source spans. Only
//! surrounding ordinary metadata is encoded/replaced; image tables stay raw.
use super::{
    index::{self, Node},
    splice, Base, PenNode, Span,
};
mod fields;
struct Protected {
    before: Span,
    after: Span,
    source: Span,
}
fn resource_field(key: &str, value: &[u8]) -> bool {
    matches!(key, "src" | "url" | "thumbnailUrl")
        || [
            b"\"src\":".as_slice(),
            b"\"url\":".as_slice(),
            b"\"thumbnailUrl\":".as_slice(),
            b"\"op-image:".as_slice(),
        ]
        .iter()
        .any(|needle| value.windows(needle.len()).any(|part| part == *needle))
}
fn field(fields: &[(&str, Span)], key: &str) -> Option<Span> {
    fields
        .iter()
        .find_map(|(name, span)| (*name == key).then_some(*span))
}
fn unique(fields: &[(&str, Span)]) -> bool {
    let mut seen = std::collections::HashSet::new();
    fields.iter().all(|(name, _)| seen.insert(*name))
}
impl Base {
    pub(super) fn patch_resource_metadata(
        &mut self,
        node: Node,
        old: &PenNode,
        current: &PenNode,
    ) -> Option<()> {
        let (before, after) = splice::resource_metadata_pair(old, current, &self.thumbnails)?;
        let previous = index::fields(
            &before,
            Span {
                start: 0,
                end: before.len(),
            },
        )?;
        let next = index::fields(
            &after,
            Span {
                start: 0,
                end: after.len(),
            },
        )?;
        if !unique(&previous) || !unique(&next) {
            return None;
        }
        let raw = fields::locate(&mut self.raw, node)?;
        let source_field = |key: &str| {
            raw.iter()
                .find_map(|(name, span)| (name == key).then_some(*span))
        };
        let mut protected = Vec::new();
        for (key, span) in &previous {
            if !resource_field(key, &before[span.start..span.end]) {
                continue;
            }
            let target = field(&next, key)?;
            // One common collision-safe collector compared the actual typed
            // sources. Equal references are not inferred from a hash alone.
            if before[span.start..span.end] != after[target.start..target.end] {
                return None;
            }
            protected.push(Protected {
                before: *span,
                after: target,
                source: source_field(key)?,
            });
        }
        if protected.is_empty() {
            return None;
        }
        // Added/removed/changed resource identities are outside this proof,
        // even when the rest of the metadata would serialize successfully.
        for (key, span) in &next {
            if resource_field(key, &after[span.start..span.end]) {
                let old = field(&previous, key)?;
                if !resource_field(key, &before[old.start..old.end])
                    || before[old.start..old.end] != after[span.start..span.end]
                {
                    return None;
                }
            }
        }
        match (
            field(&previous, "children"),
            field(&next, "children"),
            node.children,
        ) {
            (Some(a), Some(b), Some(children))
                if source_field("children") == Some(children.span) =>
            {
                protected.push(Protected {
                    before: a,
                    after: b,
                    source: children.span,
                });
            }
            (None, None, None) => (),
            _ => return None,
        }
        protected.sort_unstable_by_key(|part| part.before.start);
        let mut a = 0;
        let mut b = 0;
        let mut source = node.object.start;
        let mut edits = Vec::new();
        for part in protected {
            if part.before.start < a || part.after.start < b || part.source.start < source {
                return None;
            }
            let span = Span {
                start: source,
                end: part.source.start,
            };
            self.current_matches(span, &before[a..part.before.start])?;
            edits.push((span, after[b..part.after.start].to_vec()));
            a = part.before.end;
            b = part.after.end;
            source = part.source.end;
        }
        let span = Span {
            start: source,
            end: node.object.end,
        };
        self.current_matches(span, &before[a..])?;
        edits.push((span, after[b..].to_vec()));
        // Verify ALL old runs first. If any later reservation/overlap check
        // fails, the caller discards the entire derived chain before saving.
        for (span, bytes) in edits {
            self.replace(span, bytes)?;
        }
        Some(())
    }
}
