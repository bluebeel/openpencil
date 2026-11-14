//! Borrowed target lookup. Only requested identities retain structural paths.
use crate::{EditorState, NodeId, PenNodeExt};
use jian_ops_schema::node::PenNode;
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
};
#[derive(Clone)]
pub(super) struct Original {
    pub preorder: Box<[usize]>,
    pub sibling_first: Box<[usize]>,
    pub count: usize,
}
pub(super) struct Index {
    pub wanted: HashSet<String>,
    pub original: HashMap<String, Original>,
}
pub(super) fn walk<'a>(roots: &'a [PenNode], mut visit: impl FnMut(&'a PenNode, &[usize])) {
    let mut frames = vec![(roots, 0usize)];
    let mut path = Vec::new();
    while let Some((siblings, next)) = frames.last_mut() {
        if *next == siblings.len() {
            frames.pop();
            if !frames.is_empty() {
                path.pop();
            }
            continue;
        }
        let index = *next;
        *next += 1;
        let current = *siblings;
        let node = &current[index];
        path.push(index);
        visit(node, &path);
        if let Some(children) = node.children().filter(|children| !children.is_empty()) {
            frames.push((children, 0));
        } else {
            path.pop();
        }
    }
}
fn priority(a: &[usize], b: &[usize]) -> Ordering {
    let common = a.iter().zip(b).take_while(|(a, b)| a == b).count();
    if common == a.len() || common == b.len() {
        return a.len().cmp(&b.len());
    }
    match (a.len() == common + 1, b.len() == common + 1) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => a[common].cmp(&b[common]),
    }
}
impl Index {
    pub(super) fn new(roots: &[PenNode], wanted: HashSet<String>) -> Self {
        let mut index = Self {
            wanted,
            original: HashMap::new(),
        };
        index.populate(roots);
        index
    }
    pub(super) fn for_state(state: &EditorState, wanted: HashSet<String>) -> Self {
        if wanted.is_empty() {
            return Self {
                wanted,
                original: HashMap::new(),
            };
        }
        let scope = state.command_lookup_active_scope();
        let mut original = HashMap::with_capacity(wanted.len());
        for id in &wanted {
            let Some((preorder, sibling_first, count)) = state.command_lookup_variants(&scope, id)
            else {
                return Self::new(state.active_children(), wanted);
            };
            if count == 0 {
                continue;
            }
            let (Some(preorder), Some(sibling_first)) = (preorder, sibling_first) else {
                return Self::new(state.active_children(), wanted);
            };
            original.insert(
                id.clone(),
                Original {
                    preorder,
                    sibling_first,
                    count,
                },
            );
        }
        Self { wanted, original }
    }
    fn populate(&mut self, roots: &[PenNode]) {
        if self.wanted.is_empty() {
            return;
        }
        walk(roots, |node, path| {
            if !self.wanted.contains(node.id_str()) {
                return;
            }
            if let Some(old) = self.original.get_mut(node.id_str()) {
                old.count += 1;
                if priority(path, &old.sibling_first).is_lt() {
                    old.sibling_first = path.into();
                }
            } else {
                self.original.insert(
                    node.id_str().into(),
                    Original {
                        preorder: path.into(),
                        sibling_first: path.into(),
                        count: 1,
                    },
                );
            }
        });
    }
    pub(super) fn resolve_extra(&mut self, state: &EditorState, roots: &[PenNode], id: &NodeId) {
        if self.wanted.insert(id.as_str().into()) {
            if let Some((preorder, sibling_first, count)) =
                state.command_lookup_variants(&state.command_lookup_active_scope(), id.as_str())
            {
                if count == 0 {
                    return;
                }
                if let (Some(preorder), Some(sibling_first)) = (preorder, sibling_first) {
                    self.original.insert(
                        id.as_str().to_owned(),
                        Original {
                            preorder,
                            sibling_first,
                            count,
                        },
                    );
                    return;
                }
            }
            // Paste may resolve one additional parent identity after locating
            // its leaf anchor. This is one borrowed pass, never per payload.
            walk(roots, |node, path| {
                if node.id_str() != id.as_str() {
                    return;
                }
                if let Some(old) = self.original.get_mut(id.as_str()) {
                    old.count += 1;
                    if priority(path, &old.sibling_first).is_lt() {
                        old.sibling_first = path.into();
                    }
                } else {
                    self.original.insert(
                        id.as_str().into(),
                        Original {
                            preorder: path.into(),
                            sibling_first: path.into(),
                            count: 1,
                        },
                    );
                }
            });
        }
    }
    pub(super) fn ambiguous(&self, id: &str) -> bool {
        self.original.get(id).is_some_and(|source| source.count > 1)
    }
}
pub(super) fn at<'a>(roots: &'a [PenNode], path: &[usize]) -> Option<&'a PenNode> {
    let (head, tail) = path.split_first()?;
    let mut node = roots.get(*head)?;
    for index in tail {
        node = node.children()?.get(*index)?;
    }
    Some(node)
}
pub(super) fn at_mut<'a>(roots: &'a mut [PenNode], path: &[usize]) -> Option<&'a mut PenNode> {
    let (head, tail) = path.split_first()?;
    let mut node = roots.get_mut(*head)?;
    for index in tail {
        if node.children().is_none() {
            return None;
        }
        node = node.children_mut()?.get_mut(*index)?;
    }
    Some(node)
}

pub(super) fn relative<'a>(mut node: &'a PenNode, path: &[usize]) -> Option<&'a PenNode> {
    for index in path {
        node = node.children()?.get(*index)?;
    }
    Some(node)
}
pub(super) fn relative_mut<'a>(
    mut node: &'a mut PenNode,
    path: &[usize],
) -> Option<&'a mut PenNode> {
    for index in path {
        if node.children().is_none() {
            return None;
        }
        node = node.children_mut()?.get_mut(*index)?;
    }
    Some(node)
}
