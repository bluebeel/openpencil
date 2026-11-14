//! Bounded exact-input reuse of immutable local-coordinate gradient shaders.
//! Canvas CTM, rounded geometry, blend and clipping remain at the draw site.
use super::{gradient_color_arrays, premul_interpolation};
use op_editor_ui::Color;
use skia_safe::{Point, Shader};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    hash::{Hash, Hasher},
    mem::size_of,
};

const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_ENTRIES: usize = 256;
const MAX_STOPS: usize = 256;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Linear,
    Radial,
}
struct Request<'a> {
    kind: Kind,
    geometry: [f32; 4],
    opacity: f32,
    stops: &'a [(f32, Color)],
}
struct Key {
    kind: Kind,
    geometry: [u32; 4],
    opacity: u32,
    stops: Vec<[u32; 5]>,
}
fn stop_bits((offset, color): &(f32, Color)) -> [u32; 5] {
    [
        offset.to_bits(),
        color.r.to_bits(),
        color.g.to_bits(),
        color.b.to_bits(),
        color.a.to_bits(),
    ]
}
impl Request<'_> {
    fn hash(&self) -> u64 {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        (self.kind as u8).hash(&mut hash);
        self.geometry.map(f32::to_bits).hash(&mut hash);
        self.opacity.to_bits().hash(&mut hash);
        self.stops.len().hash(&mut hash);
        for stop in self.stops {
            stop_bits(stop).hash(&mut hash);
        }
        hash.finish()
    }
    fn matches(&self, key: &Key) -> bool {
        self.kind == key.kind
            && self.geometry.map(f32::to_bits) == key.geometry
            && self.opacity.to_bits() == key.opacity
            && self.stops.len() == key.stops.len()
            && self
                .stops
                .iter()
                .zip(&key.stops)
                .all(|(a, b)| stop_bits(a) == *b)
    }
    fn owned_key(&self) -> Key {
        Key {
            kind: self.kind,
            geometry: self.geometry.map(f32::to_bits),
            opacity: self.opacity.to_bits(),
            stops: self.stops.iter().map(stop_bits).collect(),
        }
    }
    fn build(&self) -> Option<Shader> {
        // Identical operations, float colors, interpolation and failure
        // behavior as the original uncached rectangle/path implementations.
        let (colors, offsets) = gradient_color_arrays(self.stops, self.opacity);
        let colors = skia_safe::gradient::Colors::new(
            &colors[..],
            Some(offsets.as_slice()),
            skia_safe::TileMode::Clamp,
            None,
        );
        let gradient = skia_safe::gradient::Gradient::new(colors, premul_interpolation());
        let [a, b, c, d] = self.geometry;
        match self.kind {
            Kind::Linear => skia_safe::gradient::shaders::linear_gradient(
                (Point::new(a, b), Point::new(c, d)),
                &gradient,
                None,
            ),
            Kind::Radial => skia_safe::gradient::shaders::radial_gradient(
                (Point::new(a, b), c),
                &gradient,
                None,
            ),
        }
    }
}
struct Entry {
    key: Key,
    shader: Shader,
    bytes: usize,
}
#[derive(Default)]
struct Cache {
    entries: HashMap<u64, Entry>,
    fifo: VecDeque<u64>,
    bytes: usize,
}
impl Cache {
    fn charged(&self) -> usize {
        self.bytes
            .saturating_add(size_of::<Self>())
            .saturating_add(self.entries.capacity() * (size_of::<(u64, Entry)>() + 16))
            .saturating_add(self.fifo.capacity() * size_of::<u64>())
    }
    fn insert(&mut self, hash: u64, key: Key, shader: Shader) {
        // Account owned key capacity plus a conservative native-stop charge.
        // Opaque native allocator/RSS is not claimed as an exact byte ledger;
        // both entry and per-entry stop bounds remain hard limits.
        let bytes = key.stops.capacity() * size_of::<[u32; 5]>() + key.stops.len() * 64 + 512;
        if bytes > MAX_BYTES {
            return;
        }
        if let Some(old) = self.entries.remove(&hash) {
            self.bytes -= old.bytes;
        } else {
            self.fifo.push_back(hash);
        }
        self.bytes += bytes;
        self.entries.insert(hash, Entry { key, shader, bytes });
        while self.entries.len() > MAX_ENTRIES || self.charged() > MAX_BYTES {
            let Some(old) = self.fifo.pop_front() else {
                break;
            };
            if let Some(entry) = self.entries.remove(&old) {
                self.bytes -= entry.bytes;
            }
        }
    }
}
thread_local! { static CACHE: RefCell<Cache> = RefCell::new(Cache::default()); }
fn shader(request: Request<'_>) -> Option<Shader> {
    if request.stops.len() > MAX_STOPS {
        return request.build();
    }
    let hash = request.hash();
    if let Some(shader) = CACHE.with(|cache| {
        cache
            .borrow()
            .entries
            .get(&hash)
            .filter(|entry| request.matches(&entry.key))
            .map(|entry| entry.shader.clone())
    }) {
        return Some(shader);
    }
    let shader = request.build()?;
    // Failures are not retained: a transient native allocation failure must
    // remain retryable, matching the original shader/fallback contract.
    CACHE.with(|cache| {
        cache
            .borrow_mut()
            .insert(hash, request.owned_key(), shader.clone())
    });
    Some(shader)
}
pub(crate) fn linear(
    start: Point,
    end: Point,
    stops: &[(f32, Color)],
    opacity: f32,
) -> Option<Shader> {
    shader(Request {
        kind: Kind::Linear,
        geometry: [start.x, start.y, end.x, end.y],
        stops,
        opacity,
    })
}
pub(crate) fn radial(
    center: Point,
    radius: f32,
    stops: &[(f32, Color)],
    opacity: f32,
) -> Option<Shader> {
    shader(Request {
        kind: Kind::Radial,
        geometry: [center.x, center.y, radius, 0.0],
        stops,
        opacity,
    })
}
