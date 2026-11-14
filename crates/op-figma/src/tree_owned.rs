//! Compact GUID/index-owned tree builder; source fields remain unchanged.
use super::*;

pub(super) fn build(node_changes: Vec<FigValue>) -> Option<TreeNode> {
    let mut lookup: HashMap<FigGuid, usize> = HashMap::new();
    let mut nodes: Vec<Option<FigValue>> = Vec::new();
    for node in node_changes {
        if node.get_str("phase") == Some("REMOVED") {
            continue;
        }
        let Some(key) = node.get("guid").and_then(FigGuid::from_value) else {
            continue;
        };
        if let Some(&slot) = lookup.get(&key) {
            nodes[slot] = Some(node)
        } else {
            lookup.insert(key, nodes.len());
            nodes.push(Some(node))
        }
    }
    let mut root = None;
    let mut counts = vec![0usize; nodes.len()];
    for (i, node) in nodes.iter().enumerate() {
        let n = node.as_ref().unwrap();
        if n.get_str("type") == Some("DOCUMENT") {
            root = Some(i);
            continue;
        }
        if let Some(&parent) = n
            .get("parentIndex")
            .and_then(|p| p.get("guid"))
            .and_then(FigGuid::from_value)
            .and_then(|g| lookup.get(&g))
        {
            counts[parent] += 1
        }
    }
    let root = root?;
    let mut offsets = Vec::with_capacity(nodes.len() + 1);
    offsets.push(0);
    for count in &counts {
        offsets.push(offsets.last().unwrap() + count)
    }
    let mut children = vec![0usize; *offsets.last().unwrap()];
    counts.fill(0);
    for (i, node) in nodes.iter().enumerate() {
        let n = node.as_ref().unwrap();
        if n.get_str("type") == Some("DOCUMENT") {
            continue;
        }
        if let Some(&p) = n
            .get("parentIndex")
            .and_then(|p| p.get("guid"))
            .and_then(FigGuid::from_value)
            .and_then(|g| lookup.get(&g))
        {
            children[offsets[p] + counts[p]] = i;
            counts[p] += 1
        }
    }
    drop(counts);
    drop(lookup);
    Some(materialize_index(root, &mut nodes, &offsets, &children, 0))
}
fn materialize_index(
    index: usize,
    nodes: &mut [Option<FigValue>],
    offsets: &[usize],
    edges: &[usize],
    depth: u32,
) -> TreeNode {
    let figma = nodes[index].take().unwrap_or(FigValue::Null);
    let pages = figma.get_str("type") == Some("DOCUMENT");
    let mut children = Vec::new();
    if depth < MAX_TREE_DEPTH {
        let mut sorted = edges[offsets[index]..offsets[index + 1]].to_vec();
        sorted.sort_by(|a, b| {
            let pa = nodes[*a].as_ref().map(position_of).unwrap_or("");
            let pb = nodes[*b].as_ref().map(position_of).unwrap_or("");
            if pages {
                pa.cmp(pb)
            } else {
                pb.cmp(pa)
            }
        });
        children.reserve_exact(sorted.len());
        for child in sorted {
            children.push(materialize_index(child, nodes, offsets, edges, depth + 1))
        }
    }
    TreeNode { figma, children }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn object(fields: Vec<(&str, FigValue)>) -> FigValue {
        FigValue::Object(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }
    fn node(id: u32, parent: Option<u32>, position: &str, kind: &str) -> FigValue {
        let mut value = object(vec![
            ("guid", FigValue::Guid(2, id)),
            ("type", FigValue::Str(kind.into())),
            ("name", FigValue::Str(format!("latest{id}").into())),
            (
                "unknown",
                object(vec![("data", FigValue::Bytes(vec![0, 128, 255].into()))]),
            ),
        ]);
        if let Some(parent) = parent {
            value.set(
                "parentIndex",
                object(vec![
                    ("guid", FigValue::Guid(2, parent)),
                    ("position", FigValue::Str(position.into())),
                ]),
            );
        }
        value
    }
    fn same(a: &TreeNode, b: &TreeNode) {
        assert_eq!(a.figma, b.figma);
        assert_eq!(a.children.len(), b.children.len());
        for (a, b) in a.children.iter().zip(&b.children) {
            same(a, b)
        }
    }
    #[test]
    fn indexed_tree_matches_borrowed_unknown_fields_latest_duplicates_ties_and_orphans() {
        let mut values = vec![
            node(1, None, "", "DOCUMENT"),
            node(2, Some(1), "b", "CANVAS"),
            node(3, Some(1), "a", "CANVAS"),
            node(4, Some(2), "z", "RECTANGLE"),
            node(5, Some(2), "z", "RECTANGLE"),
            node(6, Some(999), "", "RECTANGLE"),
            node(7, Some(8), "", "RECTANGLE"),
            node(8, Some(7), "", "RECTANGLE"),
        ];
        let mut replacement = node(4, Some(3), "q", "FRAME");
        replacement.set(
            "futureUnknown",
            FigValue::Array(vec![FigValue::Guid(7, 9)].into()),
        );
        values.push(replacement);
        let mut removed = node(9, Some(2), "", "RECTANGLE");
        removed.set("phase", FigValue::Str("REMOVED".into()));
        values.push(removed);
        values.push(FigValue::Null);
        same(
            &build(values.clone()).unwrap(),
            &super::super::build_tree(&values).unwrap(),
        );
        assert!(build(vec![]).is_none());
        assert!(build(vec![node(2, None, "", "CANVAS")]).is_none());
    }
}
