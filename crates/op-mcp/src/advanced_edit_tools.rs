//! Dedicated typed authoring operations without document snapshots or JSON
//! node rewrites. The host retains ordinary authority, history and commit.
use crate::{write_tools::parse_node_id, McpTool, ToolErrorCode, ToolOutcome};
use op_editor_core::{EditorCommand, EffectField, LayoutPropValue, VideoPlaybackField};
use std::collections::BTreeMap;

pub const NAMES: &[&str] = &[
    "set_effect_param",
    "set_effect_color",
    "set_image_video_src",
    "set_image_video_playback",
    "add_image_video",
    "remove_image_video",
    "set_node_layout_property",
    "replace_font_family",
];

pub fn is_local_setter(name: &str) -> bool {
    name != "replace_font_family" && NAMES.contains(&name)
}
pub fn tools(requested: Option<&str>) -> Vec<Box<dyn McpTool>> {
    NAMES
        .iter()
        .filter(|name| requested.is_none_or(|requested| requested == **name))
        .map(|name| Box::new(Edit { name }) as Box<dyn McpTool>)
        .collect()
}
struct Edit {
    name: &'static str,
}
impl McpTool for Edit {
    fn name(&self) -> &str {
        self.name
    }
    fn call(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        match self.command(args) {
            Ok(command) => ToolOutcome::OkWithCommand(
                BTreeMap::from([("wrote".into(), "true".into())]),
                command,
            ),
            Err(error) => error,
        }
    }
}
type Arguments = BTreeMap<String, String>;
fn invalid(message: impl Into<String>) -> ToolOutcome {
    ToolOutcome::Err(ToolErrorCode::InvalidArgument, message.into())
}
fn required<'a>(args: &'a Arguments, key: &str) -> Result<&'a str, ToolOutcome> {
    args.get(key).map(String::as_str).ok_or_else(|| {
        ToolOutcome::Err(ToolErrorCode::MissingArgument, format!("{key} is required"))
    })
}
fn index(args: &Arguments) -> Result<u32, ToolOutcome> {
    required(args, "index")?
        .parse()
        .map_err(|_| invalid("index must be a non-negative u32"))
}
fn finite(args: &Arguments) -> Result<f32, ToolOutcome> {
    required(args, "value")?
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| invalid("value must be a finite f32"))
}
fn boolean(args: &Arguments) -> Result<bool, ToolOutcome> {
    match required(args, "value")? {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(invalid("value must be true or false")),
    }
}
fn layout_value(raw: &str) -> Result<LayoutPropValue, ToolOutcome> {
    if raw.starts_with('[') {
        let values: Vec<f64> = serde_json::from_str(raw)
            .map_err(|_| invalid("layout value array must contain finite numbers"))?;
        if !values.iter().all(|v| v.is_finite()) {
            return Err(invalid("layout value array must contain finite numbers"));
        }
        return Ok(LayoutPropValue::NumberArray(values));
    }
    if matches!(raw, "true" | "false") {
        return Ok(LayoutPropValue::Bool(raw == "true"));
    }
    if let Ok(value) = raw.parse::<f64>() {
        return value
            .is_finite()
            .then_some(LayoutPropValue::Number(value))
            .ok_or_else(|| invalid("layout numeric value must be finite"));
    }
    if raw.starts_with('{') {
        return Err(invalid(
            "layout value must be a number, array, boolean or keyword",
        ));
    }
    Ok(LayoutPropValue::Keyword(raw.to_owned()))
}
impl Edit {
    fn command(&self, args: &Arguments) -> Result<EditorCommand, ToolOutcome> {
        if self.name == "replace_font_family" {
            let from = required(args, "from")?;
            let to = required(args, "to")?;
            if from.trim().is_empty() || to.trim().is_empty() {
                return Err(invalid("from and to must name non-empty font families"));
            }
            return Ok(EditorCommand::ReplaceFontFamily {
                from: from.into(),
                to: to.into(),
            });
        }
        let node_id = parse_node_id(args, "node_id")?;
        Ok(match self.name {
            "set_effect_param" => {
                let field = match required(args, "field")? {
                    "offsetX" => EffectField::OffsetX,
                    "offsetY" => EffectField::OffsetY,
                    "blur" => EffectField::Blur,
                    "spread" => EffectField::Spread,
                    "radius" => EffectField::Radius,
                    _ => {
                        return Err(invalid(
                            "field must be offsetX, offsetY, blur, spread or radius",
                        ))
                    }
                };
                EditorCommand::SetEffectParam {
                    node_id,
                    index: index(args)?,
                    field,
                    value: finite(args)?,
                }
            }
            "set_effect_color" => EditorCommand::SetEffectColor {
                node_id,
                index: index(args)?,
                hex: required(args, "hex")?.into(),
            },
            "set_image_video_src" => EditorCommand::SetImageVideoSrc {
                node_id,
                src: required(args, "src")?.into(),
            },
            "set_image_video_playback" => {
                let field =
                    match required(args, "field")? {
                        "autoplay" => VideoPlaybackField::Autoplay,
                        "loop" => VideoPlaybackField::Loop,
                        "muted" => VideoPlaybackField::Muted,
                        "holdLastFrame" => VideoPlaybackField::HoldLastFrame,
                        "clickToReplay" => VideoPlaybackField::ClickToReplay,
                        _ => return Err(invalid(
                            "field must be autoplay, loop, muted, holdLastFrame or clickToReplay",
                        )),
                    };
                EditorCommand::SetImageVideoPlayback {
                    node_id,
                    field,
                    value: boolean(args)?,
                }
            }
            "add_image_video" => EditorCommand::AddImageVideo { node_id },
            "remove_image_video" => EditorCommand::RemoveImageVideo { node_id },
            "set_node_layout_property" => EditorCommand::SetNodeLayoutProp {
                node_id,
                property: required(args, "property")?.into(),
                value: layout_value(required(args, "value")?)?,
            },
            _ => {
                return Err(ToolOutcome::Err(
                    ToolErrorCode::Internal,
                    "unknown typed authoring tool".into(),
                ))
            }
        })
    }
}
