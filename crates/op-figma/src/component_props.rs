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

fn assignment_target(assignment: &FigValue) -> Option<FigValue> {
    assignment
        .get("value")
        .and_then(|value| value.get("guidValue"))
        .cloned()
        .or_else(|| {
            assignment
                .get("varValue")?
                .get("value")?
                .get("symbolIdValue")?
                .get("guid")
                .cloned()
        })
}

fn assignment_value(assignment: &FigValue, field: &str) -> Option<FigValue> {
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
            })
            .cloned();
    }
    assignment
        .get("value")
        .and_then(|value| value.get_bool("boolValue"))
        .or_else(|| {
            assignment
                .get("varValue")?
                .get("value")?
                .get_bool("boolValue")
        })
        .map(FigValue::Bool)
}

#[derive(Clone)]
struct PropertyRoute {
    owner: FigValue,
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
                    if !targets.iter().any(|route| {
                        route.field == field
                            && guid_to_string(&route.owner) == guid_to_string(&owner_guid)
                    }) {
                        targets.push(PropertyRoute {
                            owner: owner_guid.clone(),
                            field,
                        });
                    }
                }
            });
        }
    }
    routes
}

fn guid_path(value: &FigValue) -> Option<Vec<FigValue>> {
    value
        .get("guidPath")?
        .get_array("guids")
        .map(<[FigValue]>::to_vec)
}

fn path_value(guids: Vec<FigValue>) -> FigValue {
    FigValue::Object(vec![("guids".into(), FigValue::Array(guids))])
}

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

fn assignment_entries(holder: &FigValue) -> Vec<FigValue> {
    let mut entries = holder
        .get("symbolData")
        .and_then(|data| data.get_array("componentPropAssignments"))
        .map(<[FigValue]>::to_vec)
        .unwrap_or_default();
    for assignment in holder.get_array("componentPropAssignments").unwrap_or(&[]) {
        let id = assignment.get("defID").and_then(guid_to_string);
        if let Some(existing) = entries
            .iter_mut()
            .find(|entry| id.is_some() && entry.get("defID").and_then(guid_to_string) == id)
        {
            *existing = assignment.clone();
        } else {
            entries.push(assignment.clone());
        }
    }
    entries
}

fn resolved_assignments(
    holder: &FigValue,
    routes: &HashMap<String, Vec<PropertyRoute>>,
) -> Vec<(FigValue, &'static str, FigValue)> {
    assignment_entries(holder)
        .iter()
        .flat_map(|assignment| {
            let targets = assignment
                .get("defID")
                .and_then(guid_to_string)
                .and_then(|id| routes.get(&id));
            targets.into_iter().flatten().filter_map(move |route| {
                Some((
                    route.owner.clone(),
                    route.field,
                    assignment_value(assignment, route.field)?,
                ))
            })
        })
        .collect()
}

fn upsert_property_override(
    overrides: &mut Vec<FigValue>,
    path: Vec<FigValue>,
    field: &str,
    target: FigValue,
) {
    if let Some(existing) = overrides.iter_mut().find(|entry| same_path(entry, &path)) {
        // Legacy direct overrides are authoritative when both encodings exist.
        if existing.get(field).is_none() {
            existing.set(field, target);
        }
        return;
    }
    overrides.push(FigValue::Object(vec![
        ("guidPath".into(), path_value(path)),
        (field.into(), target),
    ]));
}

fn normalize_node(node: &mut FigValue, routes: &HashMap<String, Vec<PropertyRoute>>) {
    let direct = resolved_assignments(node, routes);
    let node_guid = node.get("guid").and_then(guid_to_string);
    let mut direct_updates: Vec<(&'static str, FigValue)> = Vec::new();
    let mut additions = Vec::new();
    for (owner, field, target) in direct {
        if node_guid.as_deref()
            == owner
                .get("sessionID")
                .and_then(|_| guid_to_string(&owner))
                .as_deref()
        {
            if matches!(field, "visible" | "textData") {
                if let Some((_, value)) = direct_updates.iter_mut().find(|(key, _)| *key == field) {
                    *value = target;
                } else {
                    direct_updates.push((field, target));
                }
            } else if node.get(field).is_none()
                && !direct_updates.iter().any(|(key, _)| *key == field)
            {
                direct_updates.push((field, target));
            }
        } else {
            additions.push((vec![owner], field, target));
        }
    }

    // Inspect authored overrides by reference. Large documents can carry
    // deeply nested override graphs even when no property needs normalizing.
    let authored = node
        .get("symbolData")
        .and_then(|data| data.get_array("symbolOverrides"))
        .unwrap_or(&[]);
    for entry in authored {
        let assignments = resolved_assignments(entry, routes);
        if assignments.is_empty() {
            continue;
        }
        let prefix = guid_path(entry).unwrap_or_default();
        for (owner, field, target) in assignments {
            let mut path = prefix.clone();
            path.push(owner);
            additions.push((path, field, target));
        }
    }
    additions.retain(|(path, field, _)| {
        !authored
            .iter()
            .any(|entry| same_path(entry, path) && entry.get(field).is_some())
    });
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
    for (path, field, target) in additions {
        upsert_property_override(overrides, path, field, target);
    }
}

pub(crate) fn resolve_component_property_swaps(nodes: &mut [FigValue]) {
    let routes = component_property_routes(nodes);
    if routes.is_empty() {
        return;
    }
    nodes
        .iter_mut()
        .for_each(|node| normalize_node(node, &routes));
}

#[cfg(test)]
#[path = "component_props/visibility_tests.rs"]
mod visibility_tests;

#[cfg(test)]
#[path = "component_props/text_tests.rs"]
mod text_tests;

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
