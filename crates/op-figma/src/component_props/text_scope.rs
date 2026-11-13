//! A shared property id can name text layers in many component variants.
//! Only the actual instance master subtree is a target for that placement.

use super::{guid_to_string, FigValue};
use std::collections::{HashMap, HashSet};

pub(super) struct TextScopes {
    masters: HashSet<String>,
    targets: HashMap<String, String>,
    owners: HashMap<String, Vec<String>>,
}

impl TextScopes {
    pub(super) fn new(nodes: &[FigValue]) -> Self {
        let by_id: HashMap<_, _> = nodes
            .iter()
            .filter_map(|node| Some((node.get("guid").and_then(guid_to_string)?, node)))
            .collect();
        let masters: HashSet<_> = by_id
            .iter()
            .filter(|(_, node)| node.get_str("type") == Some("SYMBOL"))
            .map(|(id, _)| id.clone())
            .collect();
        let mut targets = HashMap::new();
        let mut owners = HashMap::new();
        for (id, node) in &by_id {
            if masters.contains(id) {
                targets.insert(id.clone(), id.clone());
            }
            if let Some(target) = node
                .get("overriddenSymbolID")
                .or_else(|| node.get("symbolData")?.get("symbolID"))
                .and_then(guid_to_string)
            {
                if masters.contains(&target) {
                    targets.insert(id.clone(), target);
                }
            }
            if node.get("componentPropRefs").is_none()
                && node.get("componentPropertyReferences").is_none()
            {
                continue;
            }
            let mut ancestors = Vec::new();
            let mut current = Some(id.clone());
            for _ in 0..1024 {
                let Some(key) = current.take() else {
                    break;
                };
                if masters.contains(&key) {
                    ancestors.push(key.clone());
                }
                current = by_id
                    .get(&key)
                    .and_then(|node| node.get("parentIndex"))
                    .and_then(|parent| parent.get("guid"))
                    .and_then(guid_to_string);
                if current.as_ref() == Some(id) {
                    break;
                }
            }
            owners.insert(id.clone(), ancestors);
        }
        Self {
            masters,
            targets,
            owners,
        }
    }

    pub(super) fn target(&self, holder: &FigValue) -> Option<String> {
        if let Some(target) = holder
            .get("overriddenSymbolID")
            .or_else(|| holder.get("symbolData")?.get("symbolID"))
            .and_then(guid_to_string)
        {
            return self.masters.contains(&target).then_some(target);
        }
        let id = holder.get("guid").and_then(guid_to_string).or_else(|| {
            holder
                .get("guidPath")?
                .get_array("guids")?
                .last()
                .and_then(guid_to_string)
        })?;
        self.targets.get(&id).cloned()
    }

    pub(super) fn contains(&self, master: &str, owner: &FigValue) -> bool {
        owner
            .get("sessionID")
            .and_then(|_| guid_to_string(owner))
            .and_then(|id| self.owners.get(&id))
            .is_some_and(|ancestors| ancestors.iter().any(|ancestor| ancestor == master))
    }
}
