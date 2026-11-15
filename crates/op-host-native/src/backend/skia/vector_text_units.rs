//! Integer TrueType outline points for explicitly-authored document text.
//!
//! Platform variable-font paths may keep fractional design coordinates. The
//! document outline contract rounds explicit points first, then truncates the
//! integer midpoint between consecutive off-curve points. On/off metadata is
//! read from the actual face; coincident explicit midpoints are never guessed.
use std::{cell::RefCell, collections::HashMap, rc::Rc};

use skia_safe::{Font, GlyphId, Matrix, Path, PathBuilder, PathVerb, Point, Typeface};

const MAX_PREFIX: usize = 1024 * 1024;
const MAX_POINTS: usize = 65536;

#[cfg(test)]
thread_local! { static PREFIX_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

#[derive(Clone, Copy, Debug, PartialEq)]
enum Command {
    Move(bool),
    Line,
    Quad(bool),
    Close,
}

pub(super) struct GlyphMetadata {
    commands: Vec<Command>,
}
impl GlyphMetadata {
    pub(super) fn bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.commands.capacity() * std::mem::size_of::<Command>()
    }
}

pub(super) fn outline(font: &Font, glyph: GlyphId) -> Option<Path> {
    outline_impl(font, glyph, None)
}

pub(super) fn outline_with_metadata(
    font: &Font,
    glyph: GlyphId,
    cache: Option<&RefCell<MetadataCache>>,
) -> Option<Path> {
    if cache.is_none() {
        return outline(font, glyph);
    }
    outline_impl(font, glyph, cache)
}

fn outline_impl(
    font: &Font,
    glyph: GlyphId,
    cache: Option<&RefCell<MetadataCache>>,
) -> Option<Path> {
    let face = font.typeface();
    let units = face.units_per_em()?;
    if !(16..=16384).contains(&units)
        || !font.size().is_finite()
        || font.size() <= 0.0
        || !font.scale_x().is_finite()
        || !font.skew_x().is_finite()
    {
        return None;
    }
    let metadata = if let Some(cache) = cache {
        let generation = jian_skia::font_generation();
        let key = (face.unique_id(), glyph);
        let found = cache.borrow_mut().lookup(generation, key);
        if let Some(value) = found {
            value
        } else {
            // Typeface/font work never runs under the cache's mutable borrow.
            let value = read_metadata(&face, glyph).map(Rc::new);
            cache.borrow_mut().insert(generation, key, value.clone());
            value
        }
    } else {
        super::reuse::glyph_metadata(face.unique_id(), glyph, || read_metadata(&face, glyph))
    }?;
    let mut unit_font = font.with_size(units as f32)?;
    unit_font.set_scale_x(1.0);
    unit_font.set_skew_x(0.0);
    let path = normalize(&unit_font.get_path(glyph)?, &metadata)?;
    // Store design coordinates in em before applying the requested size.
    // A reciprocal multiply preserves the outline's f32 encoding for
    // non-power-of-two UPEM values; dividing each coordinate rounds differently.
    let path = path.with_transform(&Matrix::scale((1.0 / units as f32, 1.0 / units as f32)));
    let scale = font.size();
    Some(path.with_transform(&Matrix::new_all(
        scale * font.scale_x(),
        scale * font.skew_x(),
        0.0,
        0.0,
        scale,
        0.0,
        0.0,
        0.0,
        1.0,
    )))
}

fn prefix(face: &Typeface, tag: &[u8; 4], len: usize) -> Option<Vec<u8>> {
    let tag = u32::from_be_bytes(*tag);
    if len > MAX_PREFIX || len > face.get_table_size(tag)? {
        return None;
    }
    #[cfg(test)]
    PREFIX_READS.with(|count| count.set(count.get() + 1));
    let mut data = vec![0; len];
    (face.get_table_data(tag, &mut data) == len).then_some(data)
}
fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        data.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}
fn read_metadata(face: &Typeface, glyph: GlyphId) -> Option<GlyphMetadata> {
    let head = prefix(face, b"head", 54)?;
    let format = u16_at(&head, 50)?;
    let width = match format {
        0 => 2,
        1 => 4,
        _ => return None,
    };
    let loca = prefix(face, b"loca", (usize::from(glyph) + 2) * width)?;
    let offset = |index: usize| -> Option<usize> {
        if width == 2 {
            Some(usize::from(u16_at(&loca, index * width)?) * 2)
        } else {
            Some(u32::from_be_bytes(
                loca.get(index * width..index * width + 4)?
                    .try_into()
                    .ok()?,
            ) as usize)
        }
    };
    let start = offset(usize::from(glyph))?;
    let end = offset(usize::from(glyph) + 1)?;
    if end <= start {
        return None;
    }
    // skia-safe exposes prefix reads, not offset reads. Bound that transient
    // prefix and retain only this glyph's compact command metadata. CFF,
    // composites and later glyph ranges keep their original platform path.
    let glyf = prefix(face, b"glyf", end)?;
    parse_glyph(glyf.get(start..end)?)
}

fn parse_glyph(data: &[u8]) -> Option<GlyphMetadata> {
    let contours = u16_at(data, 0)? as i16;
    if contours <= 0 {
        return None;
    }
    let mut ends = Vec::with_capacity(contours as usize);
    for i in 0..contours as usize {
        let end = usize::from(u16_at(data, 10 + 2 * i)?) + 1;
        if ends.last().is_some_and(|prior| *prior >= end) {
            return None;
        }
        ends.push(end);
    }
    let points = *ends.last()?;
    if points > MAX_POINTS {
        return None;
    }
    let instructions_at = 10 + 2 * contours as usize;
    let instructions = usize::from(u16_at(data, instructions_at)?);
    let mut cursor = instructions_at.checked_add(2)?.checked_add(instructions)?;
    if cursor > data.len() {
        return None;
    }
    let mut flags = Vec::with_capacity(points);
    while flags.len() < points {
        let flag = *data.get(cursor)?;
        cursor += 1;
        if flag & 0x80 != 0 {
            return None;
        }
        let repeat = if flag & 8 != 0 {
            let n = usize::from(*data.get(cursor)?) + 1;
            cursor += 1;
            n
        } else {
            1
        };
        if repeat > points - flags.len() {
            return None;
        }
        flags.resize(flags.len() + repeat, flag);
    }
    let coordinate_bytes: usize = flags
        .iter()
        .map(|flag| {
            let x = if flag & 2 != 0 {
                1
            } else if flag & 16 != 0 {
                0
            } else {
                2
            };
            let y = if flag & 4 != 0 {
                1
            } else if flag & 32 != 0 {
                0
            } else {
                2
            };
            x + y
        })
        .sum();
    if cursor.checked_add(coordinate_bytes)? > data.len() {
        return None;
    }
    let mut commands = Vec::with_capacity(points + 2 * contours as usize);
    let mut start = 0;
    for end in ends {
        let contour = &flags[start..end];
        let first_on = contour[0] & 1 != 0;
        let last_on = contour[contour.len() - 1] & 1 != 0;
        let implied_start = !first_on && !last_on;
        commands.push(Command::Move(implied_start));
        let sequence = if first_on {
            &contour[1..]
        } else if last_on {
            &contour[..contour.len() - 1]
        } else {
            contour
        };
        let mut off = false;
        for flag in sequence {
            if flag & 1 != 0 {
                commands.push(if off {
                    Command::Quad(false)
                } else {
                    Command::Line
                });
                off = false;
            } else {
                if off {
                    commands.push(Command::Quad(true));
                }
                off = true;
            }
        }
        if off {
            commands.push(Command::Quad(implied_start));
        }
        commands.push(Command::Close);
        start = end;
    }
    Some(GlyphMetadata { commands })
}

#[derive(Clone, Copy)]
struct Record {
    verb: PathVerb,
    points: [Point; 4],
}
fn rounded(p: Point) -> Point {
    // OpenType nearest rounding has ties toward +infinity in font y-up.
    Point::new((p.x + 0.5).floor(), -(-p.y + 0.5).floor())
}
fn midpoint(a: Point, b: Point) -> Point {
    Point::new(((a.x + b.x) * 0.5).trunc(), ((a.y + b.y) * 0.5).trunc())
}
fn normalize(path: &Path, metadata: &GlyphMetadata) -> Option<Path> {
    let records: Vec<_> = path
        .iter()
        .map(|record| {
            let mut points = [Point::default(); 4];
            points[..record.points().len()].copy_from_slice(record.points());
            Record {
                verb: record.verb(),
                points,
            }
        })
        .collect();
    if records.len() != metadata.commands.len() {
        return None;
    }
    let mut out = PathBuilder::new_with_fill_type(path.fill_type());
    let mut contour_start = 0;
    for (i, (record, command)) in records.iter().zip(&metadata.commands).enumerate() {
        if record
            .points
            .iter()
            .any(|p| !p.x.is_finite() || !p.y.is_finite())
        {
            return None;
        }
        match (record.verb, *command) {
            (PathVerb::Move, Command::Move(implied)) => {
                contour_start = i;
                let point = if implied {
                    let close = metadata.commands[i..]
                        .iter()
                        .position(|v| *v == Command::Close)?
                        + i;
                    if records.get(i + 1)?.verb != PathVerb::Quad
                        || records.get(close - 1)?.verb != PathVerb::Quad
                    {
                        return None;
                    }
                    midpoint(
                        rounded(records[i + 1].points[1]),
                        rounded(records[close - 1].points[1]),
                    )
                } else {
                    rounded(record.points[0])
                };
                out.move_to(point);
            }
            (PathVerb::Line, Command::Line) => {
                out.line_to(rounded(record.points[1]));
            }
            (PathVerb::Quad, Command::Quad(implied)) => {
                let control = rounded(record.points[1]);
                let end = if implied {
                    let next = if metadata.commands.get(i + 1) == Some(&Command::Close) {
                        contour_start + 1
                    } else {
                        i + 1
                    };
                    if records.get(next)?.verb != PathVerb::Quad {
                        return None;
                    }
                    midpoint(control, rounded(records[next].points[1]))
                } else {
                    rounded(record.points[2])
                };
                out.quad_to(control, end);
            }
            (PathVerb::Close, Command::Close) => {
                out.close();
            }
            _ => return None,
        }
    }
    Some(out.detach())
}

#[cfg(test)]
#[path = "vector_text_units_tests.rs"]
mod tests;

/// Native backend-owned metadata only: no table bytes, typefaces or paths.
/// It survives live frames, independently of the export outline cache.
pub(crate) struct MetadataCache {
    generation: u64,
    entries: HashMap<(u32, GlyphId), MetadataEntry>,
    bytes: usize,
    tick: u64,
    max_bytes: usize,
    max_entries: usize,
}
struct MetadataEntry {
    value: Option<Rc<GlyphMetadata>>,
    bytes: usize,
    used: u64,
}
impl Default for MetadataCache {
    fn default() -> Self {
        Self::new(1024 * 1024, 2048)
    }
}
impl MetadataCache {
    fn new(max_bytes: usize, max_entries: usize) -> Self {
        Self {
            generation: jian_skia::font_generation(),
            entries: HashMap::new(),
            bytes: 0,
            tick: 0,
            max_bytes,
            max_entries,
        }
    }
    fn refresh(&mut self, generation: u64) {
        if self.generation != generation {
            self.entries = HashMap::new();
            self.bytes = 0;
            self.tick = 0;
            self.generation = generation;
        }
    }
    fn lookup(
        &mut self,
        generation: u64,
        key: (u32, GlyphId),
    ) -> Option<Option<Rc<GlyphMetadata>>> {
        self.refresh(generation);
        let entry = self.entries.get_mut(&key)?;
        self.tick = self.tick.saturating_add(1);
        entry.used = self.tick;
        Some(entry.value.clone())
    }
    fn insert(&mut self, generation: u64, key: (u32, GlyphId), value: Option<Rc<GlyphMetadata>>) {
        self.refresh(generation);
        let bytes = 128 + 2 * value.as_ref().map_or(0, |value| value.bytes());
        if bytes > self.max_bytes || self.max_entries == 0 {
            return;
        }
        if let Some(old) = self.entries.remove(&key) {
            self.bytes -= old.bytes;
        }
        while self.entries.len() >= self.max_entries
            || bytes > self.max_bytes.saturating_sub(self.bytes)
        {
            let Some(key) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            let old = self.entries.remove(&key).unwrap();
            self.bytes -= old.bytes;
        }
        self.tick = self.tick.saturating_add(1);
        self.entries.insert(
            key,
            MetadataEntry {
                value,
                bytes,
                used: self.tick,
            },
        );
        self.bytes += bytes;
    }
}
