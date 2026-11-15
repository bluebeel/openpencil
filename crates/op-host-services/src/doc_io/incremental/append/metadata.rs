//! Header edits refer to the original encoded segment, never guessed output
//! coordinates. Child insertions stay inside the excluded child-array span.
use super::*;
impl Base {
    pub(in crate::doc_io::incremental) fn patch_appended(
        &mut self,
        old: &PenNode,
        current: &PenNode,
    ) -> Option<()> {
        if !eligible(current) {
            return None;
        }
        let placed = self.appends.nodes.get(old.id_str()).copied()?;
        let Source::Segment(id) = placed.source else {
            return None;
        };
        let before = splice::old_metadata(old, current.children().is_some())?;
        let after = splice::metadata(current)?;
        let previous = splice::parts(&before)?;
        let next = splice::parts(&after)?;
        match (placed.node.children, previous, next) {
            (None, None, None) => {
                self.fragment_matches(id, placed.node.object, &before)?;
                self.replace_fragment(id, placed.node.object, after)
            }
            (Some(children), Some(a), Some(b)) => {
                let prefix = Span {
                    start: placed.node.object.start,
                    end: children.span.start,
                };
                let suffix = Span {
                    start: children.span.end,
                    end: placed.node.object.end,
                };
                self.fragment_matches(id, prefix, &before[..a.start])?;
                self.fragment_matches(id, suffix, &before[a.end..])?;
                self.replace_fragment(id, prefix, after[..b.start].to_vec())?;
                self.replace_fragment(id, suffix, after[b.end..].to_vec())
            }
            _ => None,
        }
    }
    fn fragment_matches(&self, id: usize, span: Span, expected: &[u8]) -> Option<()> {
        let segment = self.appends.segments.get(id)?;
        let actual = match segment.patches.get(&span) {
            Some(bytes) => bytes.as_slice(),
            None => segment.bytes.get(span.start..span.end)?,
        };
        (actual == expected).then_some(())
    }
    fn replace_fragment(&mut self, id: usize, span: Span, bytes: Vec<u8>) -> Option<()> {
        let segment = self.appends.segments.get_mut(id)?;
        if (!segment.patches.contains_key(&span) && segment.patches.len() == MAX_IDS)
            || overlap(&segment.patches, &segment.points, span)
        {
            return None;
        }
        segment.patches.insert(span, bytes);
        (self.appends.bytes().checked_add(self.replacement_bytes)? <= EDIT_LIMIT).then_some(())
    }
}
