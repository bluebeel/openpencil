//! Worker-owned successful encoded outputs, never public mutable paint callbacks.
//! Weak scene ownership separates Arc::make_mut and prevents address recycling.
use crate::{RasterFormat, RasterSampling};
use op_editor_ui::layout_scene::{LayoutScene, NodeKind, SceneFillLayer, SceneFillType, SceneNode};
use std::{
    mem::size_of,
    sync::{Arc, Weak},
};
#[path = "shared_raster_dependencies.rs"]
mod dependencies;
const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_OUTPUT: usize = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 128;
const MAX_ID_BYTES: usize = 16 * 1024;
#[derive(Clone, Copy)]
pub(super) struct Input<'a> {
    pub page: usize,
    pub node: Option<&'a str>,
    pub format: RasterFormat,
    pub scale: f32,
    pub sampling: RasterSampling,
    pub fonts: u64,
}
struct Key {
    page: usize,
    node: Option<String>,
    format: RasterFormat,
    scale: u32,
    sampling: RasterSampling,
    fonts: u64,
}
impl Key {
    fn matches(&self, input: Input<'_>) -> bool {
        self.page == input.page
            && self.node.as_deref() == input.node
            && self.format == input.format
            && self.scale == input.scale.to_bits()
            && self.sampling == input.sampling
            && self.fonts == input.fonts
    }
}
struct Entry {
    owner: Weak<LayoutScene>,
    key: Key,
    bytes: Vec<u8>,
    charge: usize,
}
#[derive(Default)]
pub(super) struct Cache {
    entries: Vec<Entry>,
    payload: usize,
    dependencies: dependencies::Dependencies,
}
impl Cache {
    /// Call only with actual source branches borrowed from `scene`. Scene Weak
    /// identity is the authority; mutable/public paint callbacks never use it.
    pub(super) fn resource_free(&mut self, scene: &Arc<LayoutScene>, node: &SceneNode) -> bool {
        let safe = self.dependencies.get_or_check(scene, node);
        // Proof metadata shares the existing hard cache budget. A newly
        // memoized branch must not leave already-retained outputs over it.
        while self.retained() > MAX_BYTES && !self.entries.is_empty() {
            self.remove(0);
        }
        safe
    }

    fn retained(&self) -> usize {
        self.payload + self.entries.capacity() * size_of::<Entry>() + self.dependencies.retained()
    }
    fn remove(&mut self, index: usize) {
        self.payload -= self.entries.remove(index).charge;
    }
    fn refresh(&mut self, fonts: u64) {
        let mut index = 0;
        while index < self.entries.len() {
            if self.entries[index].owner.strong_count() == 0
                || self.entries[index].key.fonts != fonts
            {
                self.remove(index);
            } else {
                index += 1;
            }
        }
    }
    fn find(&mut self, scene: &Arc<LayoutScene>, input: Input<'_>) -> Option<usize> {
        self.refresh(input.fonts);
        let owner = Arc::downgrade(scene);
        self.entries
            .iter()
            .position(|entry| Weak::ptr_eq(&entry.owner, &owner) && entry.key.matches(input))
    }
    /// Metadata-only preflight; never clones one output per batch target.
    pub(super) fn contains(&mut self, scene: &Arc<LayoutScene>, input: Input<'_>) -> bool {
        self.find(scene, input).is_some()
    }
    pub(super) fn get(&mut self, scene: &Arc<LayoutScene>, input: Input<'_>) -> Option<Vec<u8>> {
        let index = self.find(scene, input)?;
        let entry = self.entries.remove(index);
        let result = entry.bytes.clone(); // the existing owned delivery buffer only
        self.entries.push(entry);
        Some(result)
    }
    pub(super) fn insert(&mut self, scene: &Arc<LayoutScene>, input: Input<'_>, encoded: &[u8]) {
        self.refresh(input.fonts);
        if encoded.len() > MAX_OUTPUT || input.node.is_some_and(|id| id.len() > MAX_ID_BYTES) {
            return;
        }
        if let Some(index) = self.find(scene, input) {
            self.remove(index);
        }
        if self.entries.capacity() == 0 && self.entries.try_reserve_exact(MAX_ENTRIES).is_err() {
            return;
        }
        let node = input.node.map(str::to_owned);
        // A retained Weak keeps its small allocation/control block, not its
        // destroyed page/node buffers. Include that allocation conservatively.
        let metadata = node.as_ref().map_or(0, String::capacity) + size_of::<LayoutScene>() + 64;
        let required = encoded.len().saturating_add(metadata);
        while !self.entries.is_empty()
            && (self.entries.len() == MAX_ENTRIES
                || self.retained() > MAX_BYTES.saturating_sub(required))
        {
            self.remove(0);
        }
        if self.retained() > MAX_BYTES.saturating_sub(required) {
            return;
        }
        let bytes = encoded.to_vec();
        let charge = bytes.capacity().saturating_add(metadata);
        if bytes.capacity() > MAX_OUTPUT || self.retained() > MAX_BYTES.saturating_sub(charge) {
            return;
        }
        self.payload += charge;
        self.entries.push(Entry {
            owner: Arc::downgrade(scene),
            key: Key {
                page: input.page,
                node,
                format: input.format,
                scale: input.scale.to_bits(),
                sampling: input.sampling,
                fonts: input.fonts,
            },
            bytes,
            charge,
        });
    }
}
fn layer_safe(layer: &SceneFillLayer) -> bool {
    match layer {
        SceneFillLayer::Solid { .. } | SceneFillLayer::Gradient { .. } => true,
        SceneFillLayer::Shader { .. } | SceneFillLayer::Image { .. } => false,
    }
}
/// A conservative exact dependency proof for a selected branch/mask source.
/// Ancestors contribute only immutable geometry/composite/blur/clip context;
/// SceneSelection::all_paint_sources visits every actually painted mask branch.
pub(super) fn resource_free(root: &SceneNode) -> bool {
    let mut pending = vec![std::slice::from_ref(root).iter()];
    while let Some(siblings) = pending.last_mut() {
        let Some(node) = siblings.next() else {
            pending.pop();
            continue;
        };
        // The actual painter returns before any resource access for the
        // complete hidden branch. Authored hidden resources remain in scene.
        if node.hidden {
            continue;
        }
        if node.image_src.is_some()
            || node.image_thumbnail_src.is_some()
            || node.image_geometry.is_some()
            || node.image_paint.is_some()
            || node.video.is_some()
            || node.widget.is_some()
            || matches!(node.kind, NodeKind::Other(_))
            || node.shader.is_some()
            || matches!(node.fill_type, SceneFillType::Image | SceneFillType::Shader)
            || !node
                .fill_layers
                .iter()
                .chain(
                    node.stroke_paint
                        .iter()
                        .flat_map(|stroke| stroke.layers.iter()),
                )
                .chain(
                    node.text_runs
                        .iter()
                        .flat_map(|run| run.fill_layers.iter().flat_map(|layers| layers.iter())),
                )
                .all(layer_safe)
        {
            return false;
        }
        if !node.children.is_empty() {
            pending.push(node.children.iter());
        }
    }
    true
}
