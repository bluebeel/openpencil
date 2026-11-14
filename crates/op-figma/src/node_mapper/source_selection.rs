//! Select complete source records before expanding their decoded payloads.
//!
//! Every record keeps its validated wire bytes until selection. Render masters
//! follow all GUID-bearing fields, including unknown containers. Global style,
//! variable, route/topology and pooled-instance evidence remain available to
//! the existing preprocessing passes; unrelated evidence is not another render.
use super::FigmaPageInfo;
use crate::{figma_types::FigGuid, kiwi::FigValue};
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub(crate) struct Topology {
    lookup: HashMap<FigGuid, usize>,
    parents: Vec<Option<usize>>,
    offsets: Vec<usize>,
    children: Vec<usize>,
    pages: Vec<usize>,
}

fn guid(node: &FigValue) -> Option<FigGuid> {
    node.get("guid").and_then(FigGuid::from_value)
}
fn active(node: &FigValue) -> bool {
    node.get_str("phase") != Some("REMOVED")
}
fn position(node: &FigValue) -> &str {
    node.get("parentIndex")
        .and_then(|p| p.get_str("position"))
        .unwrap_or("")
}

impl Topology {
    /// Ambiguous identities or cyclic/deep ancestry retain the eager reference
    /// path, including preprocessing of every historical change record.
    pub(crate) fn new(nodes: &[FigValue]) -> Option<Self> {
        let mut lookup = HashMap::new();
        for (index, node) in nodes.iter().enumerate() {
            if let Some(id) = guid(node) {
                if lookup.insert(id, index).is_some() {
                    return None;
                }
            }
        }
        let parents: Vec<_> = nodes
            .iter()
            .map(|node| {
                node.get("parentIndex")
                    .and_then(|p| p.get("guid"))
                    .and_then(FigGuid::from_value)
                    .and_then(|id| lookup.get(&id).copied())
            })
            .collect();
        let mut state = vec![0_u8; nodes.len()];
        let mut path = Vec::new();
        for start in 0..nodes.len() {
            if state[start] == 2 {
                continue;
            }
            path.clear();
            let mut at = Some(start);
            while let Some(index) = at {
                if state[index] == 2 {
                    break;
                }
                if state[index] == 1 || path.len() >= 1024 {
                    return None;
                }
                state[index] = 1;
                path.push(index);
                at = parents[index];
            }
            for index in &path {
                state[*index] = 2;
            }
        }
        let edge_parent = |i: usize| {
            let node = &nodes[i];
            if !active(node) || guid(node).is_none() || node.get_str("type") == Some("DOCUMENT") {
                return None;
            }
            parents[i].filter(|parent| active(&nodes[*parent]))
        };
        let mut counts = vec![0; nodes.len()];
        for i in 0..nodes.len() {
            if let Some(p) = edge_parent(i) {
                counts[p] += 1;
            }
        }
        let mut offsets = Vec::with_capacity(nodes.len() + 1);
        offsets.push(0);
        for count in &counts {
            offsets.push(offsets.last().unwrap() + count);
        }
        let mut children = vec![0; *offsets.last().unwrap()];
        counts.fill(0);
        for i in 0..nodes.len() {
            if let Some(p) = edge_parent(i) {
                children[offsets[p] + counts[p]] = i;
                counts[p] += 1;
            }
        }
        let root = nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                active(n) && guid(n).is_some() && n.get_str("type") == Some("DOCUMENT")
            })
            .map(|(i, _)| i)
            .last();
        let mut pages = root
            .map(|root| {
                children[offsets[root]..offsets[root + 1]]
                    .iter()
                    .copied()
                    .filter(|i| crate::tree::is_user_page_value(&nodes[*i]))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        pages.sort_by(|a, b| position(&nodes[*a]).cmp(position(&nodes[*b])));
        Some(Self {
            lookup,
            parents,
            offsets,
            children,
            pages,
        })
    }
    pub(crate) fn pages(&self, nodes: &[FigValue]) -> Vec<FigmaPageInfo> {
        self.pages
            .iter()
            .map(|i| FigmaPageInfo {
                id: guid(&nodes[*i]).map(|id| id.to_key()).unwrap_or_default(),
                name: nodes[*i].get_str("name").unwrap_or("Page").into(),
                child_count: self.offsets[i + 1] - self.offsets[*i],
            })
            .collect()
    }
    fn children(&self, index: usize) -> &[usize] {
        &self.children[self.offsets[index]..self.offsets[index + 1]]
    }
}

fn references(value: &FigValue, output: &mut Vec<FigGuid>) {
    let value = value.resolved();
    if let Some(id) = FigGuid::from_value(value) {
        output.push(id);
    }
    match value {
        FigValue::Object(fields) => {
            for (_, value) in fields {
                references(value, output);
            }
        }
        FigValue::Array(values) => {
            for value in values {
                references(value, output);
            }
        }
        _ => {}
    }
}
fn node_references(node: &FigValue, output: &mut Vec<FigGuid>) {
    if let FigValue::Object(fields) = node.resolved() {
        for (key, value) in fields {
            // Identity/ancestry is indexed independently. It must not turn a
            // page dependency into every descendant of the document root.
            if !matches!(key.as_ref(), "guid" | "parentIndex") {
                references(value, output);
            }
        }
    }
}

struct Selection<'a, F> {
    nodes: &'a mut [FigValue],
    topology: &'a Topology,
    hydrate: F,
    full: Vec<bool>,
    render: Vec<bool>,
    expanded: Vec<bool>,
    ancestors: Vec<bool>,
    pending: Vec<usize>,
    masters: HashSet<FigGuid>,
    instances: HashMap<FigGuid, Vec<usize>>,
}
impl<F: FnMut(usize, &mut FigValue)> Selection<'_, F> {
    fn full(&mut self, index: usize) {
        if !self.full[index] {
            (self.hydrate)(index, &mut self.nodes[index]);
            self.full[index] = true;
        }
    }
    fn mark(&mut self, index: usize) {
        self.full(index);
        if !self.render[index] {
            self.render[index] = true;
            self.pending.push(index);
        }
    }
    fn subtree(&mut self, root: usize) {
        let mut stack = vec![root];
        while let Some(index) = stack.pop() {
            if self.expanded[index] {
                continue;
            }
            self.expanded[index] = true;
            self.mark(index);
            stack.extend_from_slice(self.topology.children(index));
        }
    }
    fn auxiliary(&mut self, index: usize) {
        let mut at = Some(index);
        while let Some(index) = at {
            if self.ancestors[index] {
                break;
            }
            self.ancestors[index] = true;
            self.full(index);
            at = self.topology.parents[index];
        }
    }
    fn resolve(&mut self) {
        let mut ids = Vec::new();
        while let Some(index) = self.pending.pop() {
            ids.clear();
            node_references(&self.nodes[index], &mut ids);
            for id in &ids {
                if let Some(&target) = self.topology.lookup.get(id) {
                    if self.nodes[target].get_str("type") == Some("SYMBOL") {
                        self.subtree(target);
                    } else {
                        // Font restoration can look up an individual text
                        // record by GUID even outside the rendered masters.
                        self.auxiliary(target);
                    }
                }
            }
            if self.nodes[index].get_str("type") == Some("SYMBOL") {
                if let Some(id) = guid(&self.nodes[index]) {
                    if self.masters.insert(id) {
                        if let Some(instances) = self.instances.get(&id).cloned() {
                            for instance in instances {
                                self.auxiliary(instance);
                            }
                        }
                    }
                }
            }
            if let Some(parent) = self.topology.parents[index] {
                self.auxiliary(parent);
            }
        }
    }
}

pub(crate) fn select(
    mut nodes: Vec<FigValue>,
    topology: &Topology,
    page: usize,
    mut hydrate: impl FnMut(usize, &mut FigValue),
) -> Vec<FigValue> {
    let Some(&selected) = topology.pages.get(page) else {
        for (i, node) in nodes.iter_mut().enumerate() {
            hydrate(i, node);
        }
        return nodes;
    };
    let mut instances: HashMap<_, Vec<_>> = HashMap::new();
    let mut globals = Vec::new();
    for (i, node) in nodes.iter().enumerate() {
        if node.get("styleType").is_some()
            || matches!(node.get_str("type"), Some("VARIABLE" | "VARIABLE_SET"))
        {
            globals.push(i);
        }
        if active(node) && node.get_str("type") == Some("INSTANCE") {
            if let Some(id) = node
                .get("symbolData")
                .and_then(|s| s.get("symbolID"))
                .and_then(FigGuid::from_value)
            {
                instances.entry(id).or_default().push(i);
            }
        }
    }
    let count = nodes.len();
    let mut selection = Selection {
        nodes: &mut nodes,
        topology,
        hydrate,
        full: vec![false; count],
        render: vec![false; count],
        expanded: vec![false; count],
        ancestors: vec![false; count],
        pending: Vec::new(),
        masters: HashSet::new(),
        instances,
    };
    selection.subtree(selected);
    for index in globals {
        selection.mark(index);
    }
    selection.resolve();
    nodes
}

#[cfg(test)]
#[path = "source_selection_tests.rs"]
mod tests;
