//! Export-policy entries share the query cache's byte/retired-pin ledger, but
//! never alias interactive policy. Capture does not wait for an existing build.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportScenePolicy {
    ActiveCanvas,
    ActiveItems(Vec<String>),
    ActivePresentation,
    FullDocument,
    FullContentActive,
    Item(String),
}
impl ExportScenePolicy {
    pub(super) fn normalized(self, state: &EditorState) -> Self {
        match self {
            Self::ActiveItems(mut ids) => {
                ids.sort_unstable();
                ids.dedup();
                if crate::selected_scene::active_can_push_down(state, &ids) {
                    Self::ActiveItems(ids)
                } else {
                    Self::ActiveCanvas
                }
            }
            other => other,
        }
    }
    pub(super) fn bytes(&self) -> usize {
        match self {
            Self::Item(id) => id.capacity(),
            Self::ActiveItems(ids) => {
                ids.capacity() * size_of::<String>()
                    + ids.iter().map(String::capacity).sum::<usize>()
            }
            _ => 0,
        }
    }
    pub(super) fn build(&self, state: &EditorState) -> LayoutScene {
        match self {
            Self::ActiveCanvas => crate::editor_state_to_active_page_layout_scene(state),
            Self::ActiveItems(ids) => crate::editor_state_to_active_items_layout_scene(state, ids),
            Self::ActivePresentation => {
                crate::editor_state_to_active_page_export_layout_scene(state)
            }
            Self::FullDocument => crate::editor_state_to_layout_scene(state),
            Self::FullContentActive => {
                crate::editor_state_to_active_page_full_content_export_layout_scene(state)
            }
            Self::Item(id) => crate::editor_state_to_item_layout_scene(state, id),
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct ExportSceneUnavailable;
impl std::fmt::Display for ExportSceneUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the immutable export scene build did not complete; retry the request")
    }
}
impl std::error::Error for ExportSceneUnavailable {}

enum Captured {
    Ready(Arc<LayoutScene>),
    Pending(Arc<BuildSlot>),
}
/// A detached same-version scene, or a build ticket resolved AFTER the host
/// releases its document lock. The producer's exact result stays shared even
/// when too large for retained-cache admission; it is active request storage.
pub struct CachedExportScene(Captured);
impl CachedExportScene {
    pub fn resolve(&self) -> Result<Arc<LayoutScene>, ExportSceneUnavailable> {
        match &self.0 {
            Captured::Ready(scene) => Ok(Arc::clone(scene)),
            Captured::Pending(slot) => match slot.wait() {
                Some(Value::Scene(scene)) => Ok(scene),
                _ => Err(ExportSceneUnavailable),
            },
        }
    }
}

/// Trusted host capture only. A cold producer derives once under authority;
/// an existing producer is returned without waiting. Public fresh builders
/// remain unchanged, and no EditorState/PenDocument clone is introduced.
pub fn prepare_export_scene_cached(
    state: &EditorState,
    policy: ExportScenePolicy,
) -> CachedExportScene {
    let policy = policy.normalized(state);
    let selected = matches!(
        policy,
        ExportScenePolicy::ActivePresentation | ExportScenePolicy::FullContentActive
    )
    .then(|| {
        (state.selection_count() == 1 && state.selection.anchor.is_real())
            .then(|| state.selection.anchor.to_string())
    })
    .flatten();
    let key = SceneInputs::capture(state).map(|input| Key::Export {
        input,
        policy: policy.clone(),
        selected,
    });
    let Some(key) = key.filter(|key| key.bytes() <= MAX_KEY_BYTES) else {
        record_uncached();
        return CachedExportScene(Captured::Ready(Arc::new(policy.build(state))));
    };
    let key = Arc::new(key);
    let mut dead = Vec::new();
    let mut slots = Vec::new();
    enum Choice {
        Hit(Value),
        Pending(Arc<BuildSlot>),
        Build(Arc<BuildSlot>),
        Fresh,
    }
    let choice = {
        let mut cache = store().lock().unwrap_or_else(|error| error.into_inner());
        cache.purge(&mut dead, &mut slots);
        if let Some(value) = cache.hit(&key) {
            Choice::Hit(value)
        } else if let Some(slot) = cache.pending_hit(&key, &mut slots) {
            cache.coalesced = cache.coalesced.saturating_add(1);
            Choice::Pending(slot)
        } else if cache.pending.len() < MAX_BUILDS
            && cache
                .charged_bytes()
                .saturating_add(key.bytes() + size_of::<BuildSlot>() + 32)
                <= MAX_QUERY_SCENE_CACHE_BYTES
        {
            let slot = Arc::new(BuildSlot {
                key: Arc::clone(&key),
                state: Mutex::new(BuildState::Building),
                wake: Condvar::new(),
            });
            cache.pending.push(Pending {
                slot: Arc::downgrade(&slot),
                key_bytes: key.bytes(),
            });
            cache.builds = cache.builds.saturating_add(1);
            Choice::Build(slot)
        } else {
            cache.uncached = cache.uncached.saturating_add(1);
            Choice::Fresh
        }
    };
    drop(dead);
    drop(slots);
    match choice {
        Choice::Hit(Value::Scene(scene)) => CachedExportScene(Captured::Ready(scene)),
        Choice::Pending(slot) => CachedExportScene(Captured::Pending(slot)),
        Choice::Build(slot) => {
            let mut guard = BuildGuard {
                slot,
                finished: false,
            };
            let value = Value::Scene(Arc::new(policy.build(state)));
            if !admit(key, &value) {
                record_uncached();
            }
            guard.slot.publish(value);
            guard.finished = true;
            CachedExportScene(Captured::Pending(Arc::clone(&guard.slot)))
        }
        Choice::Fresh => CachedExportScene(Captured::Ready(Arc::new(policy.build(state)))),
        Choice::Hit(Value::Marquee(_) | Value::Geometry(_) | Value::Snap(_) | Value::Hit(_)) => {
            unreachable!("export scene key cannot name a query index")
        }
    }
}
