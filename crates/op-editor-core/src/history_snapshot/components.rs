//! Component metadata and complete recursively-shared roots.
use super::*;
#[derive(Debug, Clone, PartialEq)]
pub struct SharedComponent {
    pub id: NodeId,
    pub name: String,
    pub root: Arc<SharedNode>,
}
impl SharedComponent {
    pub fn storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + 2 * std::mem::size_of::<usize>()
    }
    fn capture(component: &Component, anchor: Option<&Arc<Self>>) -> Arc<Self> {
        let root = SharedNode::capture(&component.root, anchor.map(|c| &c.root));
        if let Some(old) = anchor {
            if old.id == component.id && old.name == component.name && Arc::ptr_eq(&old.root, &root)
            {
                return old.clone();
            }
        }
        let Component { id, name, root: _ } = component;
        Arc::new(Self {
            id: id.clone(),
            name: name.clone(),
            root,
        })
    }
    fn materialize(&self) -> Component {
        Component {
            id: self.id.clone(),
            name: self.name.clone(),
            root: self.root.materialize(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SharedComponents {
    components: Arc<Vec<Arc<SharedComponent>>>,
    document_backing: DocumentComponentBacking,
}
impl SharedComponents {
    pub fn capture(lib: &ComponentLibrary, anchor: Option<&Self>) -> Self {
        let previous = anchor.map(|a| a.components.as_slice()).unwrap_or(&[]);
        let mut by_id = None;
        let components = lib
            .components
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let old = previous.get(i).filter(|old| old.id == c.id).or_else(|| {
                    by_id
                        .get_or_insert_with(|| {
                            previous
                                .iter()
                                .map(|old| (&old.id, old))
                                .collect::<HashMap<_, _>>()
                        })
                        .get(&c.id)
                        .copied()
                });
                SharedComponent::capture(c, old)
            })
            .collect();
        Self {
            components: super::edges::reuse(components, anchor.map(|a| &a.components)),
            document_backing: lib.document_backing(),
        }
    }
    pub fn materialize(&self) -> ComponentLibrary {
        let mut library = ComponentLibrary::default();
        library.components = self.components.iter().map(|c| c.materialize()).collect();
        library.restore_document_backing(self.document_backing.clone());
        library
    }
    pub fn components(&self) -> &[Arc<SharedComponent>] {
        &self.components
    }
    pub fn edge_storage(&self) -> (usize, usize) {
        super::edges::storage(&self.components)
    }
    pub fn backing_identity(&self) -> usize {
        Arc::as_ptr(&self.document_backing) as usize
    }
    /// Conservative owned index storage; no canonical roots are held here.
    pub fn backing_storage_bytes(&self) -> usize {
        std::mem::size_of_val(self.document_backing.as_ref())
            + 2 * std::mem::size_of::<usize>()
            + self.document_backing.capacity()
                * (std::mem::size_of::<(NodeId, crate::component_backing::DocumentNodeLocation)>()
                    + 16)
            + self
                .document_backing
                .iter()
                .map(|(id, location)| {
                    id.as_str().len() + location.child_path.len() * std::mem::size_of::<usize>()
                })
                .sum::<usize>()
    }
}
