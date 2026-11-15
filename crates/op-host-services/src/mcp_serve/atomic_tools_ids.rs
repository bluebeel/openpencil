//! Exact request-local presence marks avoid cloning a complete id set per step.

use std::collections::HashMap;

use jian_ops_schema::{node::PenNode, PenDocument};
use op_editor_core::PenNodeExt;

use super::{BatchError, Result};

pub(super) struct Tracker {
    marks: HashMap<String, usize>,
    epoch: usize,
}

impl Tracker {
    pub(super) fn new(document: &PenDocument) -> Self {
        let mut marks = HashMap::new();
        visit(document, |id| {
            marks.insert(id.to_owned(), 0);
        });
        Self { marks, epoch: 0 }
    }

    pub(super) fn created(&mut self, document: &PenDocument) -> Result<Vec<String>> {
        let before = self.epoch;
        self.epoch += 1;
        let current = self.epoch;
        let mut created = Vec::new();
        visit(document, |id| match self.marks.get_mut(id) {
            Some(mark) => {
                if *mark != before && *mark != current {
                    created.push(id.to_owned());
                }
                *mark = current;
            }
            None => {
                created.push(id.to_owned());
                self.marks.insert(id.to_owned(), current);
            }
        });
        if created.len() > 16384 {
            return Err(BatchError::argument(
                "one operation introduces more than 16384 identities",
            ));
        }
        Ok(created)
    }
}

fn visit(document: &PenDocument, mut action: impl FnMut(&str)) {
    fn forest<'a>(roots: &'a [PenNode], action: &mut impl FnMut(&str)) {
        let mut stack = vec![roots.iter()];
        while let Some(siblings) = stack.last_mut() {
            if let Some(node) = siblings.next() {
                if !node.id_str().is_empty() {
                    action(node.id_str());
                }
                if let Some(children) = node.children() {
                    stack.push(children.iter());
                }
            } else {
                stack.pop();
            }
        }
    }
    if let Some(pages) = &document.pages {
        for page in pages {
            if !page.id.is_empty() {
                action(&page.id);
            }
            forest(&page.children, &mut action);
        }
    }
    forest(&document.children, &mut action);
}
