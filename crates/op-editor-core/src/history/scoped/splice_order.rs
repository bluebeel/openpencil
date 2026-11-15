//! Stable insertion order across selective undo branches. Missing anchors are
//! virtual vertices; current foreign siblings retain their complete order.
use super::ScopedHistoryError;
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap, VecDeque},
};
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Anchors {
    pub ids: Vec<String>,
    pub left: Option<String>,
    pub right: Option<String>,
    pub required: bool,
}
#[derive(Debug, Clone, Copy)]
pub(super) enum Token {
    Existing(usize),
    Restore(usize),
}
struct Vertex {
    output: Option<Token>,
    rank: usize,
    outgoing: Option<usize>,
    incoming_head: Option<usize>,
    incoming: usize,
}
struct Edge {
    from: usize,
    to: usize,
    next_out: Option<usize>,
    next_in: Option<usize>,
}
struct Graph {
    nodes: Vec<Vertex>,
    ids: HashMap<String, Option<usize>>,
    edges: Vec<Edge>,
}
impl Graph {
    fn node(&mut self, output: Option<Token>, rank: usize) -> usize {
        let id = self.nodes.len();
        self.nodes.push(Vertex {
            output,
            rank,
            outgoing: None,
            incoming_head: None,
            incoming: 0,
        });
        id
    }
    fn id(&mut self, id: &str) -> Result<usize, ScopedHistoryError> {
        match self.ids.get(id) {
            Some(Some(index)) => Ok(*index),
            Some(None) => Err(ScopedHistoryError::NodePathChanged),
            None => {
                let index = self.node(None, usize::MAX - 2);
                self.ids.insert(id.to_owned(), Some(index));
                Ok(index)
            }
        }
    }
    fn edge(&mut self, a: usize, b: usize) {
        if a == b {
            return;
        }
        let index = self.edges.len();
        self.edges.push(Edge {
            from: a,
            to: b,
            next_out: self.nodes[a].outgoing,
            next_in: self.nodes[b].incoming_head,
        });
        self.nodes[a].outgoing = Some(index);
        self.nodes[b].incoming_head = Some(index);
        self.nodes[b].incoming += 1;
    }
}
pub(super) fn order(
    existing: &[String],
    restore: &[String],
    anchors: impl Iterator<Item = Anchors>,
) -> Result<Vec<Token>, ScopedHistoryError> {
    let capacity = existing.len() + restore.len() + 2;
    let mut graph = Graph {
        nodes: Vec::with_capacity(capacity),
        ids: HashMap::with_capacity(capacity),
        edges: Vec::with_capacity(capacity),
    };
    let start = graph.node(None, 0);
    let end = graph.node(None, usize::MAX);
    let mut previous = start;
    for (index, id) in existing.iter().enumerate() {
        let vertex = graph.node(Some(Token::Existing(index)), index.saturating_add(1));
        match graph.ids.entry(id.clone()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(Some(vertex));
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                entry.insert(None);
            }
        }
        graph.edge(previous, vertex);
        previous = vertex;
    }
    graph.edge(previous, end);
    let mut restored = Vec::with_capacity(restore.len());
    for (index, id) in restore.iter().enumerate() {
        if graph.ids.contains_key(id) {
            return Err(ScopedHistoryError::NodePathChanged);
        }
        let vertex = graph.node(Some(Token::Restore(index)), usize::MAX - 1);
        graph.ids.insert(id.clone(), Some(vertex));
        restored.push(vertex);
    }
    for anchor in anchors {
        let ambiguous = anchor
            .ids
            .iter()
            .chain(anchor.left.iter())
            .chain(anchor.right.iter())
            .any(|id| graph.ids.get(id).is_some_and(Option::is_none));
        if ambiguous {
            if anchor.required {
                return Err(ScopedHistoryError::NodePathChanged);
            } else {
                continue;
            }
        }
        let mut previous = match anchor.left {
            Some(id) => graph.id(&id)?,
            None => start,
        };
        for id in anchor.ids {
            let current = graph.id(&id)?;
            graph.edge(previous, current);
            previous = current;
        }
        let right = match anchor.right {
            Some(id) => graph.id(&id)?,
            None => end,
        };
        graph.edge(previous, right);
    }
    // A disconnected historical gap has no surviving placement witness.
    let mut connected = vec![false; graph.nodes.len()];
    let mut pending = VecDeque::from([start, end]);
    for (index, node) in graph.nodes.iter().enumerate() {
        if matches!(node.output, Some(Token::Existing(_))) {
            pending.push_back(index);
        }
    }
    while let Some(at) = pending.pop_front() {
        if connected[at] {
            continue;
        }
        connected[at] = true;
        let mut edge = graph.nodes[at].outgoing;
        while let Some(index) = edge {
            pending.push_back(graph.edges[index].to);
            edge = graph.edges[index].next_out;
        }
        let mut edge = graph.nodes[at].incoming_head;
        while let Some(index) = edge {
            pending.push_back(graph.edges[index].from);
            edge = graph.edges[index].next_in;
        }
    }
    if restored.iter().any(|at| !connected[*at]) {
        return Err(ScopedHistoryError::NodePathChanged);
    }
    let mut ready = BinaryHeap::new();
    for (index, node) in graph.nodes.iter().enumerate() {
        if node.incoming == 0 {
            ready.push(Reverse((node.rank, index)));
        }
    }
    let mut visited = 0;
    let mut output = Vec::with_capacity(existing.len() + restore.len());
    while let Some(Reverse((_, at))) = ready.pop() {
        visited += 1;
        if let Some(token) = graph.nodes[at].output {
            output.push(token);
        }
        let mut edge = graph.nodes[at].outgoing;
        while let Some(index) = edge {
            let next = graph.edges[index].to;
            graph.nodes[next].incoming -= 1;
            if graph.nodes[next].incoming == 0 {
                ready.push(Reverse((graph.nodes[next].rank, next)));
            }
            edge = graph.edges[index].next_out;
        }
    }
    if visited != graph.nodes.len() || output.len() != existing.len() + restore.len() {
        return Err(ScopedHistoryError::NodePathChanged);
    }
    Ok(output)
}
