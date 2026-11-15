use super::error::{IoOperation, RenderFigError};
use super::selector::Selector;
use std::io::Read;
use std::path::Path;

pub struct Arguments {
    pub input: String,
    pub page: String,
    pub output: String,
    pub fonts: Vec<String>,
    pub selector: Selector,
    pub scale: f32,
    pub phase_events: bool,
    pub authored_bounds: bool,
    pub raster_sampling: op_host_services::export::RasterSampling,
    expected_bounds: Option<[f64; 4]>,
    expected_pixels: Option<(u32, u32)>,
}
impl Arguments {
    pub fn parse(args: Vec<String>) -> Result<Self, RenderFigError> {
        if args.len() < 4 {
            return Err(RenderFigError::Usage {
                provided: args.len(),
            });
        }
        let mut result = Self {
            input: args[0].clone(),
            page: args[1].clone(),
            output: args[3].clone(),
            fonts: Vec::new(),
            selector: Selector::top_level(args[2].clone()),
            scale: 1.0,
            phase_events: false,
            authored_bounds: false,
            raster_sampling: op_host_services::export::RasterSampling::Native,
            expected_bounds: None,
            expected_pixels: None,
        };
        let mut selector_set = false;
        let mut index = 4;
        while index < args.len() {
            let option = args[index].as_str();
            if option == "--phase-events" {
                result.phase_events = true;
                index += 1;
                continue;
            }
            if option == "--authored-bounds" {
                result.authored_bounds = true;
                index += 1;
                continue;
            }
            if !option.starts_with("--") {
                result.fonts.push(args[index].clone());
                index += 1;
                continue;
            }
            let value = args
                .get(index + 1)
                .ok_or_else(|| RenderFigError::MissingOptionValue {
                    option: option.into(),
                })?;
            match option {
                "--node-path" | "--node-id" => {
                    if selector_set {
                        return Err(RenderFigError::ConflictingSelectors);
                    }
                    result.selector = if option == "--node-path" {
                        Selector::path_json(value)?
                    } else {
                        Selector::Id(value.clone())
                    };
                    selector_set = true;
                }
                "--expect-bounds" => {
                    let bounds: [f64; 4] =
                        serde_json::from_str(value).map_err(RenderFigError::BoundsParse)?;
                    if bounds.iter().any(|v| !v.is_finite()) || bounds[2] <= 0.0 || bounds[3] <= 0.0
                    {
                        return Err(RenderFigError::InvalidExpectedBounds { bounds });
                    }
                    result.expected_bounds = Some(bounds);
                }
                "--expect-size" => {
                    let (width, height) =
                        value
                            .split_once('x')
                            .ok_or_else(|| RenderFigError::SizeSyntax {
                                value: value.clone(),
                            })?;
                    let width =
                        width
                            .parse::<u32>()
                            .map_err(|source| RenderFigError::DimensionParse {
                                axis: "width",
                                value: width.into(),
                                source,
                            })?;
                    let height =
                        height
                            .parse::<u32>()
                            .map_err(|source| RenderFigError::DimensionParse {
                                axis: "height",
                                value: height.into(),
                                source,
                            })?;
                    if width == 0 || height == 0 {
                        return Err(RenderFigError::ZeroPixels { width, height });
                    }
                    result.expected_pixels = Some((width, height));
                }
                "--scale" => {
                    let scale =
                        value
                            .parse::<f32>()
                            .map_err(|source| RenderFigError::ScaleParse {
                                value: value.clone(),
                                source,
                            })?;
                    if !scale.is_finite() || !(0.5..=8.0).contains(&scale) {
                        return Err(RenderFigError::ScaleOutOfRange { scale });
                    }
                    result.scale = scale;
                }
                "--raster-sampling" => {
                    result.raster_sampling = op_host_services::export::RasterSampling::parse(value)?
                }
                "--font" => result.fonts.push(value.clone()),
                _ => {
                    return Err(RenderFigError::UnknownOption {
                        option: option.into(),
                    });
                }
            }
            index += 2;
        }
        Ok(result)
    }
    pub fn check_bounds(&self, actual: [f64; 4]) -> Result<(), RenderFigError> {
        if actual.iter().any(|v| !v.is_finite()) || actual[2] <= 0.0 || actual[3] <= 0.0 {
            return Err(RenderFigError::InvalidPaintedBounds { actual });
        }
        if self.expected_bounds.is_some_and(|expected| {
            expected
                .iter()
                .zip(actual)
                .any(|(a, b)| (a - b).abs() > 0.02)
        }) {
            return Err(RenderFigError::BoundsMismatch {
                expected: self.expected_bounds.unwrap(),
                actual,
                tolerance: 0.02,
            });
        }
        Ok(())
    }
    pub fn check_pixels(&self, actual: (u32, u32)) -> Result<(), RenderFigError> {
        if self
            .expected_pixels
            .is_some_and(|expected| expected != actual)
        {
            return Err(RenderFigError::PixelMismatch {
                expected: self.expected_pixels.unwrap(),
                actual,
            });
        }
        Ok(())
    }
}
pub fn png_dimensions(path: &Path) -> Result<(u32, u32), RenderFigError> {
    let mut header = [0u8; 24];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|source| {
            RenderFigError::io(IoOperation::ReadPng, path.display().to_string(), source)
        })?;
    if &header[..8] != b"\x89PNG\r\n\x1a\n" || &header[12..16] != b"IHDR" {
        return Err(RenderFigError::InvalidPng {
            path: path.display().to_string(),
        });
    }
    Ok((
        u32::from_be_bytes(header[16..20].try_into().unwrap()),
        u32::from_be_bytes(header[20..24].try_into().unwrap()),
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raster_sampling_is_opt_in_and_unknown_modes_are_errors() {
        assert_eq!(
            Arguments::parse(args(&[])).unwrap().raster_sampling,
            op_host_services::export::RasterSampling::Native
        );
        assert_eq!(
            Arguments::parse(args(&["--raster-sampling", "correlated4x2"]))
                .unwrap()
                .raster_sampling,
            op_host_services::export::RasterSampling::Correlated4x2
        );
        assert!(Arguments::parse(args(&["--raster-sampling", "unknown"])).is_err());
    }
    fn args(extra: &[&str]) -> Vec<String> {
        ["file.fig", "page", "card", "output.png"]
            .iter()
            .chain(extra)
            .map(|s| s.to_string())
            .collect()
    }
    #[test]
    fn expected_geometry_and_pixel_mismatches_fail() {
        let args = Arguments::parse(args(&[
            "--expect-bounds",
            "[0,0,100,80]",
            "--expect-size",
            "100x80",
        ]))
        .unwrap();
        assert!(args.check_bounds([0.0, 0.0, 100.0, 80.0]).is_ok());
        assert!(args.check_bounds([0.0, 0.0, 101.0, 80.0]).is_err());
        assert!(args.check_pixels((80, 100)).is_err());
        assert!(args.check_pixels((100, 80)).is_ok());
    }
    #[test]
    fn legacy_fonts_remain_supported_and_invalid_flags_fail_before_loading() {
        assert_eq!(
            Arguments::parse(args(&["font.ttf"])).unwrap().fonts,
            vec!["font.ttf"]
        );
        assert!(
            Arguments::parse(args(&["--phase-events"]))
                .unwrap()
                .phase_events
        );
        for extra in [
            vec!["--node-id", "a", "--node-path", "[\"card\"]"],
            vec!["--scale", "0.1"],
            vec!["--expect-size", "0x10"],
            vec!["--unknown", "x"],
        ] {
            assert!(Arguments::parse(args(&extra)).is_err());
        }
    }
}
