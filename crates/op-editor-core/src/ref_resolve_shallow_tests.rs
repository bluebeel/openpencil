use super::*;

// The former traversal clones the whole non-ref subtree before replacing its
// cloned descendants. Keep it here as an independent metadata/override contract.
fn legacy(
    nodes: &[PenNode],
    lookup: &NodeLookup<'_>,
    visited: &mut BTreeSet<String>,
) -> Vec<PenNode> {
    let mut out = Vec::new();
    for node in nodes {
        match node {
            PenNode::Ref(reference) => {
                if visited.contains(&reference.target) {
                    continue;
                }
                let Some(component) = lookup.find(&reference.target) else {
                    continue;
                };
                visited.insert(reference.target.clone());
                if let Some(resolved) = expand_ref(node, component, lookup, visited) {
                    out.push(resolved);
                }
                visited.remove(&reference.target);
            }
            other => {
                let mut clone = other.clone();
                if let Some(children) = crate::pen_node_ext::PenNodeExt::children_mut(&mut clone) {
                    *children = legacy(children, lookup, visited);
                }
                out.push(clone);
            }
        }
    }
    out
}

#[test]
fn shallow_ref_resolution_preserves_ancestor_metadata_inline_overrides_and_missing_children() {
    let doc=jian_ops_schema::load_str(r##"{
     "version":"1.0.0","pages":[
      {"id":"library","name":"L","children":[
       {"id":"master","type":"frame","reusable":true,"width":80,"height":40,"children":[
        {"id":"inner","type":"group","children":[{"id":"label","type":"text","content":"Master"}]},
        {"id":"cycle","type":"ref","ref":"master"}]}]},
      {"id":"p","name":"P","children":[
       {"id":"shell","type":"frame","name":"Keep parent","screen":"/home","slot":["body"],
        "imageSearchQuery":"forest","state":{},"bindings":{},"events":{},"lifecycle":{},"semantics":{},
        "rotation":13,"opacity":0.7,"width":300,"height":100,"fill":[{"type":"solid","color":"#3366ff"}],
        "children":[
         {"id":"none","type":"frame"},{"id":"empty","type":"group","children":[]},
         {"id":"instance","type":"ref","ref":"master","x":12,"descendants":{
           "inner":{"opacity":0.5},"label":{"content":"Overridden"}}},
         {"id":"missing","type":"ref","ref":"missing-master"}]}]}]
    }"##).unwrap().value;
    let before = doc.clone();
    let roots = &doc.pages.as_ref().unwrap()[1].children;
    let lookup = NodeLookup::from_document(&doc);
    assert_eq!(
        resolve_refs_for_canvas_roots(roots, &doc),
        legacy(roots, &lookup, &mut BTreeSet::new())
    );
    assert_eq!(doc, before, "borrowed canonical input must not change");
}
