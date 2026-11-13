//! Resolve color-variable bindings before conversion drops the decoded graph.
//! Bound colors are authoritative over cached fallback paint colors. Missing
//! variables, unavailable modes and alias cycles keep that fallback intact.
//! Shared component-master paints retain their cached colors: their instance
//! placement modes are only known later, during component expansion.

use crate::kiwi::FigValue;
use crate::tree::guid_to_string;
use std::collections::{HashMap, HashSet};

type Modes = HashMap<String, String>;

struct Variable {
    collection: Option<String>,
    values: Vec<FigValue>,
}

struct NodeModes {
    parent: Option<String>,
    own: Modes,
    master: bool,
}

fn shared_master_branch(
    key: &str,
    nodes: &HashMap<String, NodeModes>,
    cache: &mut HashMap<String, bool>,
    visiting: &mut HashSet<String>,
) -> bool {
    if let Some(shared) = cache.get(key) {
        return *shared;
    }
    // An invalid parent cycle cannot establish a safe placement context.
    if !visiting.insert(key.into()) || visiting.len() > 1024 {
        return true;
    }
    let shared = nodes.get(key).is_some_and(|node| {
        node.master
            || node
                .parent
                .as_ref()
                .is_some_and(|parent| shared_master_branch(parent, nodes, cache, visiting))
    });
    visiting.remove(key);
    cache.insert(key.into(), shared);
    shared
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
                    .and_then(guid_to_string)?,
                entry.get("variableModeID").and_then(guid_to_string)?,
            ))
        })
        .collect()
}

fn inherited_modes(
    key: &str,
    nodes: &HashMap<String, NodeModes>,
    cache: &mut HashMap<String, Modes>,
    visiting: &mut HashSet<String>,
) -> Modes {
    if let Some(modes) = cache.get(key) {
        return modes.clone();
    }
    if !visiting.insert(key.into()) || visiting.len() > 1024 {
        return Modes::new();
    }
    let Some(node) = nodes.get(key) else {
        visiting.remove(key);
        return Modes::new();
    };
    let mut modes = node
        .parent
        .as_ref()
        .map(|parent| inherited_modes(parent, nodes, cache, visiting))
        .unwrap_or_default();
    modes.extend(node.own.clone());
    visiting.remove(key);
    cache.insert(key.into(), modes.clone());
    modes
}

struct Resolver {
    variables: HashMap<String, Variable>,
    defaults: Modes,
}

impl Resolver {
    fn color(
        &self,
        binding: &FigValue,
        modes: &Modes,
        visiting: &mut HashSet<String>,
    ) -> Option<FigValue> {
        let value = binding.get("value")?;
        if let Some(color) = value.get("colorValue") {
            color.get_f64("r")?;
            color.get_f64("g")?;
            color.get_f64("b")?;
            return Some(color.clone());
        }
        let key = value.get("alias")?.get("guid").and_then(guid_to_string)?;
        if visiting.len() >= 128 || !visiting.insert(key.clone()) {
            return None;
        }
        let variable = self.variables.get(&key)?;
        let mode = variable.collection.as_ref().and_then(|collection| {
            modes
                .get(collection)
                .or_else(|| self.defaults.get(collection))
        });
        let entry = match mode {
            Some(mode) => variable
                .values
                .iter()
                .find(|entry| entry.get("modeID").and_then(guid_to_string).as_ref() == Some(mode)),
            None if variable.values.len() == 1 => variable.values.first(),
            None => None,
        }?;
        self.color(entry.get("variableData")?, modes, visiting)
    }

    fn resolve_value(&self, value: &mut FigValue, inherited: &Modes) {
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

    fn resolve_fields(&self, value: &mut FigValue, modes: &Modes) {
        if let Some(color) = value
            .get("colorVar")
            .and_then(|binding| self.color(binding, modes, &mut HashSet::new()))
        {
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
    };
    let mut node_modes = HashMap::new();
    for node in nodes.iter() {
        let Some(key) = node.get("guid").and_then(guid_to_string) else {
            continue;
        };
        node_modes.insert(
            key.clone(),
            NodeModes {
                parent: node
                    .get("parentIndex")
                    .and_then(|parent| parent.get("guid"))
                    .and_then(guid_to_string),
                own: local_modes(node),
                master: node.get_str("type") == Some("SYMBOL"),
            },
        );
        match node.get_str("type") {
            Some("VARIABLE") if node.get_str("variableResolvedType") == Some("COLOR") => {
                resolver.variables.insert(
                    key,
                    Variable {
                        collection: node
                            .get("variableSetID")
                            .and_then(|set| set.get("guid"))
                            .and_then(guid_to_string),
                        values: node
                            .get("variableDataValues")
                            .and_then(|values| values.get_array("entries"))
                            .unwrap_or(&[])
                            .to_vec(),
                    },
                );
            }
            Some("VARIABLE_SET") => {
                // Figma's first collection mode is the default; stored mode
                // arrays may arrive unsorted, so use their authored position.
                if let Some(mode) = node.get_array("variableSetModes").and_then(|modes| {
                    modes
                        .iter()
                        .min_by_key(|mode| mode.get_str("sortPosition").unwrap_or(""))
                }) {
                    if let Some(mode) = mode.get("id").and_then(guid_to_string) {
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
    let mut cache = HashMap::new();
    let mut shared = HashMap::new();
    for node in nodes.iter_mut() {
        if matches!(node.get_str("type"), Some("VARIABLE" | "VARIABLE_SET")) {
            continue;
        }
        let key = node.get("guid").and_then(guid_to_string);
        if key.as_ref().is_some_and(|key| {
            shared_master_branch(key, &node_modes, &mut shared, &mut HashSet::new())
        }) {
            continue;
        }
        let modes = key
            .map(|key| inherited_modes(&key, &node_modes, &mut cache, &mut HashSet::new()))
            .unwrap_or_default();
        resolver.resolve_value(node, &modes);
    }
}

#[cfg(test)]
#[path = "color_variables/tests.rs"]
mod tests;
