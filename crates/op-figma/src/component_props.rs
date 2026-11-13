//! Normalize Figma component-property assignments into legacy symbol overrides.
//!
//! Modern `.fig` files store instance swaps, visibility and text assignments
//! keyed by a component-property definition id. The component master separately
//! marks the layers and fields that each definition controls. The
//! rest of the importer already understands explicit symbol overrides, so this
//! pass joins those two records before tree construction.

use crate::kiwi::FigValue;
use crate::tree::guid_to_string;
use std::collections::HashMap;

mod path_index;
use path_index::{PathIndex, PathKey};
mod text_scope;
use text_scope::TextScopes;

fn assignment_target(assignment: &FigValue) -> Option<&FigValue> {
    assignment
        .get("value")
        .and_then(|value| value.get("guidValue"))
        .or_else(|| {
            assignment
                .get("varValue")?
                .get("value")?
                .get("symbolIdValue")?
                .get("guid")
        })
}

fn assignment_value<'a>(assignment: &'a FigValue, field: &str) -> Option<&'a FigValue> {
    if field == "overriddenSymbolID" {
        return assignment_target(assignment);
    }
    if field == "textData" {
        return assignment
            .get("value")
            .and_then(|value| value.get("textValue"))
            .filter(|value| value.get_str("characters").is_some())
            .or_else(|| {
                assignment
                    .get("varValue")?
                    .get("value")?
                    .get("textDataValue")
                    .filter(|value| value.get_str("characters").is_some())
            });
    }
    assignment
        .get("value")
        .and_then(|value| value.get("boolValue"))
        .filter(|value| value.as_bool().is_some())
        .or_else(|| {
            assignment
                .get("varValue")?
                .get("value")?
                .get("boolValue")
                .filter(|value| value.as_bool().is_some())
        })
}

#[derive(Clone)]
struct PropertyRoute {
    owner: FigValue,
    owner_key: Option<String>,
    field: &'static str,
}

fn visit_references(value: &FigValue, visit: &mut impl FnMut(&FigValue)) {
    match value {
        FigValue::Array(values) => values
            .iter()
            .for_each(|value| visit_references(value, visit)),
        FigValue::Object(pairs) => {
            if value.get("defID").is_some() {
                visit(value);
            } else {
                pairs
                    .iter()
                    .for_each(|(_, value)| visit_references(value, visit));
            }
        }
        _ => {}
    }
}

fn component_property_routes(nodes: &[FigValue]) -> HashMap<String, Vec<PropertyRoute>> {
    let mut routes: HashMap<String, Vec<PropertyRoute>> = HashMap::new();
    for node in nodes {
        let Some(owner_guid) = node.get("guid").cloned() else {
            continue;
        };
        let owner_key = guid_to_string(&owner_guid);
        for key in ["componentPropRefs", "componentPropertyReferences"] {
            let Some(references) = node.get(key) else {
                continue;
            };
            visit_references(references, &mut |reference| {
                let field = reference
                    .get_str("componentPropNodeField")
                    .or_else(|| reference.get_str("type"));
                let field = match field {
                    Some("OVERRIDDEN_SYMBOL_ID" | "INSTANCE_SWAP") => "overriddenSymbolID",
                    Some("VISIBLE" | "VISIBILITY") => "visible",
                    Some("TEXT_DATA" | "TEXT") => "textData",
                    _ => return,
                };
                if let Some(def_id) = reference.get("defID").and_then(guid_to_string) {
                    let targets = routes.entry(def_id).or_default();
                    if !targets
                        .iter()
                        .any(|route| route.field == field && route.owner_key == owner_key)
                    {
                        targets.push(PropertyRoute {
                            owner: owner_guid.clone(),
                            owner_key: owner_key.clone(),
                            field,
                        });
                    }
                }
            });
        }
    }
    routes
}

fn guid_path(value: &FigValue) -> Option<&[FigValue]> {
    value.get("guidPath")?.get_array("guids")
}

fn path_value(guids: Vec<FigValue>) -> FigValue {
    FigValue::Object(vec![("guids".into(), FigValue::Array(guids))])
}

#[cfg(test)]
fn same_path(entry: &FigValue, path: &[FigValue]) -> bool {
    let Some(existing) = entry
        .get("guidPath")
        .and_then(|value| value.get_array("guids"))
    else {
        return false;
    };
    existing.len() == path.len()
        && existing.iter().zip(path).all(|(left, right)| {
            left.get_f64("sessionID") == right.get_f64("sessionID")
                && left.get_f64("localID") == right.get_f64("localID")
        })
}

fn assignment_entries(holder: &FigValue) -> Vec<&FigValue> {
    let mut entries = holder
        .get("symbolData")
        .and_then(|data| data.get_array("componentPropAssignments"))
        .map(|entries| entries.iter().collect::<Vec<_>>())
        .unwrap_or_default();
    for assignment in holder.get_array("componentPropAssignments").unwrap_or(&[]) {
        let id = assignment.get("defID").and_then(guid_to_string);
        if let Some(existing) = entries
            .iter_mut()
            .find(|entry| id.is_some() && entry.get("defID").and_then(guid_to_string) == id)
        {
            *existing = assignment;
        } else {
            entries.push(assignment);
        }
    }
    entries
}

fn visit_assignments<'a>(
    holder: &'a FigValue,
    routes: &'a HashMap<String, Vec<PropertyRoute>>,
    scopes: &TextScopes,
    mut visit: impl FnMut(&'a FigValue, Option<&'a str>, &'static str, &'a FigValue),
) {
    let assignments = assignment_entries(holder);
    if assignments.is_empty() {
        return;
    }
    let master = scopes.target(holder);
    for assignment in assignments {
        let targets = assignment
            .get("defID")
            .and_then(guid_to_string)
            .and_then(|id| routes.get(&id));
        for route in targets.into_iter().flatten() {
            let outside = master.as_ref().is_some_and(|master| {
                if route.field == "textData" {
                    route
                        .owner_key
                        .as_deref()
                        .is_none_or(|owner| !scopes.contains(master, owner))
                } else {
                    route
                        .owner_key
                        .as_deref()
                        .is_some_and(|owner| scopes.excludes_known_owner(master, owner))
                }
            });
            if outside {
                continue;
            }
            if let Some(target) = assignment_value(assignment, route.field) {
                visit(
                    &route.owner,
                    route.owner_key.as_deref(),
                    route.field,
                    target,
                );
            }
        }
    }
}

fn normalize_node(
    node: &mut FigValue,
    routes: &HashMap<String, Vec<PropertyRoute>>,
    scopes: &TextScopes,
) {
    let authored = node
        .get("symbolData")
        .and_then(|data| data.get_array("symbolOverrides"))
        .unwrap_or(&[]);
    let mut paths = PathIndex::authored(authored);
    let node_guid = node.get("guid").and_then(guid_to_string);
    let mut direct_updates: Vec<(&'static str, FigValue)> = Vec::new();
    let mut additions = Vec::new();
    visit_assignments(node, routes, scopes, |owner, owner_key, field, target| {
        if node_guid.as_deref() == owner_key {
            if matches!(field, "visible" | "textData") {
                if let Some((_, value)) = direct_updates.iter_mut().find(|(key, _)| *key == field) {
                    *value = target.clone();
                } else {
                    direct_updates.push((field, target.clone()));
                }
            } else if node.get(field).is_none()
                && !direct_updates.iter().any(|(key, _)| *key == field)
            {
                direct_updates.push((field, target.clone()));
            }
        } else {
            let key = PathKey::new(std::iter::once(owner));
            if paths.accepts(key.as_ref(), field) {
                additions.push((vec![owner.clone()], key, field, target.clone()));
            }
        }
    });
    for entry in authored {
        let prefix = guid_path(entry).unwrap_or(&[]);
        visit_assignments(entry, routes, scopes, |owner, _owner_key, field, target| {
            let key = PathKey::new(prefix.iter().chain(std::iter::once(owner)));
            if paths.accepts(key.as_ref(), field) {
                let mut path = prefix.to_vec();
                path.push(owner.clone());
                additions.push((path, key, field, target.clone()));
            }
        });
    }
    direct_updates.retain(|(field, target)| node.get(field) != Some(target));
    if direct_updates.is_empty() && additions.is_empty() {
        return;
    }
    for (field, target) in direct_updates {
        node.set(field, target);
    }
    if additions.is_empty() {
        return;
    }
    let Some(symbol_data) = node.get_mut("symbolData") else {
        return;
    };
    if symbol_data.get_array("symbolOverrides").is_none() {
        symbol_data.set("symbolOverrides", FigValue::Array(Vec::new()));
    }
    let Some(FigValue::Array(overrides)) = symbol_data.get_mut("symbolOverrides") else {
        return;
    };
    for (path, key, field, target) in additions {
        paths.apply(overrides, key, path, field, target);
    }
}

pub(crate) fn resolve_component_property_swaps(nodes: &mut [FigValue]) {
    let routes = component_property_routes(nodes);
    if routes.is_empty() {
        return;
    }
    let scopes = TextScopes::new(nodes);
    nodes
        .iter_mut()
        .for_each(|node| normalize_node(node, &routes, &scopes));
}

#[cfg(test)]
#[path = "component_props/visibility_tests.rs"]
mod visibility_tests;

#[cfg(test)]
#[path = "component_props/text_tests.rs"]
mod text_tests;

#[cfg(test)]
#[path = "component_props/scope_tests.rs"]
mod scope_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(pairs: Vec<(&str, FigValue)>) -> FigValue {
        FigValue::Object(
            pairs
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        )
    }

    fn guid(session_id: u32, local_id: u32) -> FigValue {
        obj(vec![
            ("sessionID", FigValue::Uint(session_id)),
            ("localID", FigValue::Uint(local_id)),
        ])
    }

    fn reference(owner: (u32, u32), def: (u32, u32)) -> FigValue {
        obj(vec![
            ("guid", guid(owner.0, owner.1)),
            (
                "componentPropRefs",
                FigValue::Array(vec![obj(vec![
                    ("defID", guid(def.0, def.1)),
                    (
                        "componentPropNodeField",
                        FigValue::Str("OVERRIDDEN_SYMBOL_ID".into()),
                    ),
                ])]),
            ),
        ])
    }

    #[test]
    fn turns_nested_component_assignment_into_explicit_swap_override() {
        let mut nodes = vec![
            reference((18, 15), (90, 1)),
            obj(vec![
                ("guid", guid(3, 30)),
                (
                    "symbolData",
                    obj(vec![(
                        "symbolOverrides",
                        FigValue::Array(vec![obj(vec![
                            ("guidPath", path_value(vec![guid(7, 70)])),
                            (
                                "componentPropAssignments",
                                FigValue::Array(vec![obj(vec![
                                    ("defID", guid(90, 1)),
                                    ("value", obj(vec![("guidValue", guid(44, 440))])),
                                ])]),
                            ),
                        ])]),
                    )]),
                ),
            ]),
        ];

        resolve_component_property_swaps(&mut nodes);

        let overrides = nodes[1]
            .get("symbolData")
            .and_then(|data| data.get_array("symbolOverrides"))
            .expect("normalized overrides");
        let swap = overrides
            .iter()
            .find(|entry| same_path(entry, &[guid(7, 70), guid(18, 15)]))
            .expect("nested swap route");
        assert_eq!(
            swap.get("overriddenSymbolID").and_then(guid_to_string),
            Some("44:440".into())
        );
    }

    #[test]
    fn reads_variable_symbol_value_and_preserves_direct_legacy_swap() {
        let mut nodes = vec![
            reference((18, 15), (90, 1)),
            obj(vec![
                ("guid", guid(18, 15)),
                ("overriddenSymbolID", guid(1, 10)),
                (
                    "componentPropAssignments",
                    FigValue::Array(vec![obj(vec![
                        ("defID", guid(90, 1)),
                        (
                            "varValue",
                            obj(vec![(
                                "value",
                                obj(vec![("symbolIdValue", obj(vec![("guid", guid(2, 20))]))]),
                            )]),
                        ),
                    ])]),
                ),
            ]),
        ];

        resolve_component_property_swaps(&mut nodes);

        assert_eq!(
            nodes[1].get("overriddenSymbolID").and_then(guid_to_string),
            Some("1:10".into())
        );
    }
}
