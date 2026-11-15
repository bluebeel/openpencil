//! Bounded dependency proofs for branches borrowed from one immutable scene.
//! No output bytes, resources, mutable nodes or strong scene owners are kept.
use op_editor_ui::layout_scene::{LayoutScene, SceneNode};
use std::{
    mem::size_of,
    sync::{Arc, Weak},
};
const MAX_ENTRIES: usize = 128;
const MAX_BYTES: usize = 64 * 1024;
struct Entry {
    owner: Weak<LayoutScene>,
    node: usize,
    safe: bool,
}
#[derive(Default)]
pub(super) struct Dependencies {
    entries: Vec<Entry>,
}
impl Dependencies {
    pub(super) fn retained(&self) -> usize {
        self.entries.capacity() * size_of::<Entry>()
            + self.entries.len() * (size_of::<LayoutScene>() + 64)
    }
    pub(super) fn get_or_check(&mut self, scene: &Arc<LayoutScene>, node: &SceneNode) -> bool {
        self.entries.retain(|entry| entry.owner.strong_count() != 0);
        let owner = Arc::downgrade(scene);
        let key = std::ptr::from_ref(node) as usize;
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.node == key && Weak::ptr_eq(&entry.owner, &owner))
        {
            let entry = self.entries.remove(index);
            let safe = entry.safe;
            self.entries.push(entry);
            return safe;
        }
        let safe = super::resource_free(node);
        if self.entries.capacity() == 0 {
            if self.entries.try_reserve_exact(MAX_ENTRIES).is_err() {
                return safe;
            }
            if self.retained() > MAX_BYTES {
                self.entries = Vec::new();
                return safe;
            }
        }
        let charge = size_of::<LayoutScene>() + 64;
        while !self.entries.is_empty()
            && (self.entries.len() == MAX_ENTRIES
                || self.retained() > MAX_BYTES.saturating_sub(charge))
        {
            self.entries.remove(0);
        }
        if self.retained() <= MAX_BYTES.saturating_sub(charge) {
            self.entries.push(Entry {
                owner,
                node: key,
                safe,
            });
        }
        safe
    }
}
