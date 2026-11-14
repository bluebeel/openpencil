//! Resolve color-variable bindings before conversion drops the decoded graph.
//! Bound colors are authoritative over cached fallback paint colors. Missing
//! variables, unavailable modes and alias cycles keep that fallback intact.
//! Shared component-master paints retain their cached colors: their instance
//! placement modes are only known later, during component expansion.

use crate::figma_types::FigGuid;
use crate::kiwi::FigValue;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::Arc;

type Modes = HashMap<FigGuid, FigGuid>;

struct Variable {
    collection: Option<FigGuid>,
    values: Vec<FigValue>,
}

struct NodeModes {
    parent: Option<NonZeroUsize>,
    own: Option<Box<Modes>>,
    master: bool,
}
fn shared_master_branch(
    key: usize,
    nodes: &[NodeModes],
    cache: &mut [Option<bool>],
    visiting: &mut HashSet<usize>,
) -> bool {
    if let Some(shared) = cache[key] {
        return shared;
    }
    if !visiting.insert(key) || visiting.len() > 1024 {
        return true;
    }
    let node = &nodes[key];
    let shared = node.master
        || node
            .parent
            .is_some_and(|p| shared_master_branch(p.get() - 1, nodes, cache, visiting));
    visiting.remove(&key);
    cache[key] = Some(shared);
    shared
}
fn map_bound<K, V>(map: &HashMap<K, V>) -> usize {
    if map.capacity() == 0 {
        0
    } else {
        map.capacity().next_power_of_two() * 2 * (std::mem::size_of::<(K, V)>() + 1) + 16
    }
}
fn modes_bound(modes: &Modes) -> usize {
    map_bound(modes)
}
fn resolver_bound(resolver: &Resolver) -> usize {
    std::mem::size_of::<Resolver>()
        + 16
        + map_bound(&resolver.variables)
        + modes_bound(&resolver.defaults)
        + resolver
            .variables
            .values()
            .map(|var| {
                var.values.capacity() * std::mem::size_of::<FigValue>()
                    + var
                        .values
                        .iter()
                        .map(crate::instance::value_retained_bound)
                        .sum::<usize>()
            })
            .sum::<usize>()
}

fn local_modes(value: &FigValue) -> Modes {
    value
        .get("variableModeBySetMap")
        .and_then(|map| map.get_array("entries"))
        .unwrap_or(&[])
        .iter()
        .filter_map(|entry| {
            Some((
                entry
                    .get("variableSetID")?
                    .get("guid")
                    .and_then(FigGuid::from_value)?,
                entry.get("variableModeID").and_then(FigGuid::from_value)?,
            ))
        })
        .collect()
}

fn inherited_modes(
    key: usize,
    nodes: &[NodeModes],
    cache: &mut [Option<Arc<Modes>>],
    visiting: &mut HashSet<usize>,
    empty: &Arc<Modes>,
) -> Arc<Modes> {
    if let Some(modes) = &cache[key] {
        return modes.clone();
    }
    if !visiting.insert(key) || visiting.len() > 1024 {
        return empty.clone();
    }
    let node = &nodes[key];
    let mut modes = node
        .parent
        .map(|p| inherited_modes(p.get() - 1, nodes, cache, visiting, empty))
        .unwrap_or_else(|| empty.clone());
    if let Some(own) = &node.own {
        Arc::make_mut(&mut modes).extend(own.iter().map(|(k, v)| (*k, *v)));
    }
    visiting.remove(&key);
    cache[key] = Some(modes.clone());
    modes
}

struct Resolver {
    variables: HashMap<FigGuid, Variable>,
    defaults: Modes,
    retained_bound: usize,
}

impl Resolver {
    fn color(
        &self,
        binding: &FigValue,
        modes: &Modes,
        visiting: &mut HashSet<FigGuid>,
    ) -> Option<FigValue> {
        let value = binding.get("value")?;
        if let Some(color) = value.get("colorValue") {
            color.get_f64("r")?;
            color.get_f64("g")?;
            color.get_f64("b")?;
            return Some(color.clone());
        }
        let key = value
            .get("alias")?
            .get("guid")
            .and_then(FigGuid::from_value)?;
        if visiting.len() >= 128 || !visiting.insert(key) {
            return None;
        }
        let variable = self.variables.get(&key)?;
        let mode = variable.collection.as_ref().and_then(|collection| {
            modes
                .get(collection)
                .or_else(|| self.defaults.get(collection))
        });
        let entry = match mode {
            Some(mode) => variable.values.iter().find(|entry| {
                entry.get("modeID").and_then(FigGuid::from_value).as_ref() == Some(mode)
            }),
            None if variable.values.len() == 1 => variable.values.first(),
            None => None,
        }?;
        self.color(entry.get("variableData")?, modes, visiting)
    }

    fn resolve_value(self: &Arc<Self>, value: &mut FigValue, inherited: &Modes) {
        if let FigValue::Deferred(deferred) = value {
            let resolver = self.clone();
            let modes = inherited.clone();
            crate::kiwi::deferred::Deferred::map(
                deferred,
                self.retained_bound + modes_bound(&modes),
                move |value| resolver.resolve_value(value, &modes),
            );
            return;
        }
        if !matches!(value, FigValue::Array(_) | FigValue::Object(_)) {
            return;
        }
        let own = local_modes(value);
        if own.is_empty() {
            self.resolve_fields(value, inherited);
            return;
        }
        let mut modes = inherited.clone();
        modes.extend(own);
        self.resolve_fields(value, &modes);
    }

    fn resolve_fields(self: &Arc<Self>, value: &mut FigValue, modes: &Modes) {
        if let Some(mut color) = value
            .get("colorVar")
            .and_then(|binding| self.color(binding, modes, &mut HashSet::new()))
        {
            if value.get_str("type") == Some("SOLID") {
                // A solid paint stores the bound RGBA's alpha in `opacity`,
                // with an opaque RGB `color`. Keeping that cached opacity and
                // copying RGBA into `color` would apply the alpha twice. Write
                // both fields so a different variable mode can change alpha.
                let alpha = color.get("a").cloned().unwrap_or(FigValue::Float(1.0));
                color.set("a", FigValue::Float(1.0));
                value.set("opacity", alpha);
            }
            value.set("color", color);
        }
        match value {
            FigValue::Array(values) => {
                for child in values {
                    self.resolve_value(child, modes);
                }
            }
            FigValue::Object(fields) => {
                for (key, child) in fields {
                    // Skip variable definitions and binding expressions themselves.
                    if !matches!(
                        key.as_ref(),
                        "colorVar"
                            | "variableDataValues"
                            | "variableConsumptionMap"
                            | "parameterConsumptionMap"
                            | "derivedTextData"
                            | "fillGeometry"
                            | "strokeGeometry"
                            | "vectorData"
                            | "vectorNetwork"
                            | "pluginData"
                    ) {
                        self.resolve_value(child, modes);
                    }
                }
            }
            _ => {}
        }
    }
}

pub(crate) fn resolve_color_variables(nodes: &mut [FigValue]) {
    let mut resolver = Resolver {
        variables: HashMap::new(),
        defaults: Modes::new(),
        retained_bound: 0,
    };
    for node in nodes.iter() {
        let Some(key) = node.get("guid").and_then(FigGuid::from_value) else {
            continue;
        };
        match node.get_str("type") {
            Some("VARIABLE") if node.get_str("variableResolvedType") == Some("COLOR") => {
                resolver.variables.insert(
                    key,
                    Variable {
                        collection: node
                            .get("variableSetID")
                            .and_then(|s| s.get("guid"))
                            .and_then(FigGuid::from_value),
                        values: node
                            .get("variableDataValues")
                            .and_then(|v| v.get_array("entries"))
                            .unwrap_or(&[])
                            .to_vec(),
                    },
                );
            }
            Some("VARIABLE_SET") => {
                if let Some(mode) = node.get_array("variableSetModes").and_then(|m| {
                    m.iter()
                        .min_by_key(|m| m.get_str("sortPosition").unwrap_or(""))
                }) {
                    if let Some(mode) = mode.get("id").and_then(FigGuid::from_value) {
                        resolver.defaults.insert(key, mode);
                    }
                }
            }
            _ => {}
        }
    }
    if resolver.variables.is_empty() {
        return;
    }
    let by_id: HashMap<_, _> = nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| Some((n.get("guid").and_then(FigGuid::from_value)?, i)))
        .collect();
    let node_modes: Vec<_> = nodes
        .iter()
        .map(|n| {
            let modes = local_modes(n);
            NodeModes {
                parent: n
                    .get("parentIndex")
                    .and_then(|p| p.get("guid"))
                    .and_then(FigGuid::from_value)
                    .and_then(|key| by_id.get(&key))
                    .and_then(|i| NonZeroUsize::new(i + 1)),
                own: (!modes.is_empty()).then(|| Box::new(modes)),
                master: n.get_str("type") == Some("SYMBOL"),
            }
        })
        .collect();
    resolver.retained_bound = resolver_bound(&resolver);
    let resolver = Arc::new(resolver);
    let mut cache = vec![None; nodes.len()];
    let mut shared = vec![None; nodes.len()];
    let empty = Arc::new(Modes::new());
    for node in nodes.iter_mut() {
        if matches!(node.get_str("type"), Some("VARIABLE" | "VARIABLE_SET")) {
            continue;
        }
        let key = node
            .get("guid")
            .and_then(FigGuid::from_value)
            .and_then(|id| by_id.get(&id))
            .copied();
        if key
            .is_some_and(|i| shared_master_branch(i, &node_modes, &mut shared, &mut HashSet::new()))
        {
            continue;
        }
        let modes = key
            .map(|i| inherited_modes(i, &node_modes, &mut cache, &mut HashSet::new(), &empty))
            .unwrap_or_else(|| empty.clone());
        resolver.resolve_value(node, &modes);
    }
}

#[cfg(test)]
#[path = "color_variables/tests.rs"]
mod tests;
