//! Immutable coverage reuse: exact geometry/camera/clip/sample key, no paint.
use skia_safe::{Data, IRect, Matrix, Path, Rect, Shader};
use std::{
    cell::RefCell,
    hash::{Hash, Hasher},
};
const MAX_ENTRIES: usize = 64;
const PROBATION_ENTRIES: usize = 128;
// Includes bounded native image/shader/data handle overhead, apart from pixels
// and the serialized key. Allocator/driver retention is measured separately.
const ENTRY_ALLOWANCE: usize = 1024;
const CONTAINER_ALLOWANCE: usize = 256;

pub(super) struct Key {
    path: Data,
    matrix: [u32; 9],
    clip: [i32; 4],
    grid: u32,
    hash: u64,
}
impl Key {
    pub(super) fn new(path: &Path, matrix: &Matrix, clip: IRect, grid: u32) -> Self {
        let mut key = Self {
            path: path.serialize(),
            matrix: std::array::from_fn(|i| matrix[i].to_bits()),
            clip: [clip.left, clip.top, clip.right, clip.bottom],
            grid,
            hash: 0,
        };
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        key.path.as_bytes().hash(&mut hash);
        key.matrix.hash(&mut hash);
        key.clip.hash(&mut hash);
        key.grid.hash(&mut hash);
        key.hash = hash.finish();
        key
    }
    pub(super) fn bytes(&self) -> usize {
        self.path.len()
    }
    fn matches(&self, other: &Self) -> bool {
        self.hash == other.hash
            && self.matrix == other.matrix
            && self.clip == other.clip
            && self.grid == other.grid
            && self.path.as_bytes() == other.path.as_bytes()
    }
}
struct Entry {
    key: Key,
    shader: Shader,
    bounds: Rect,
    bytes: usize,
}
#[derive(Default)]
struct Cache {
    entries: Vec<Entry>,
    payload_bytes: usize,
    probation: Vec<u64>,
}
impl Cache {
    fn bytes(&self) -> usize {
        if self.entries.capacity() == 0 && self.probation.capacity() == 0 {
            0
        } else {
            CONTAINER_ALLOWANCE
                + self.entries.capacity() * std::mem::size_of::<Entry>()
                + self.probation.capacity() * std::mem::size_of::<u64>()
                + self.payload_bytes
        }
    }
    fn trim(&mut self, budget: usize) {
        while self.bytes() > budget && !self.entries.is_empty() {
            self.payload_bytes -= self.entries.remove(0).bytes;
        }
        if self.entries.is_empty() && self.bytes() > budget {
            self.entries = Vec::new();
            self.probation = Vec::new();
        }
    }
    fn find(&mut self, key: &Key) -> Option<(Shader, Rect)> {
        let at = self.entries.iter().position(|e| e.key.matches(key))?;
        let e = self.entries.remove(at);
        let result = (e.shader.clone(), e.bounds);
        self.entries.push(e);
        Some(result)
    }
    fn repeated(&mut self, hash: u64, budget: usize) -> bool {
        // This hash controls admission only. A collision may retain an
        // unhelpful mask; find() still checks every exact input before reuse.
        if let Some(at) = self.probation.iter().position(|old| *old == hash) {
            self.probation.remove(at);
            self.probation.push(hash);
            return true;
        }
        let bytes = PROBATION_ENTRIES * std::mem::size_of::<u64>();
        if CONTAINER_ALLOWANCE + bytes > budget {
            return false;
        }
        if self.probation.capacity() == 0 {
            self.trim(budget - bytes);
            if self.probation.try_reserve_exact(PROBATION_ENTRIES).is_err() {
                return false;
            }
        }
        self.trim(budget);
        if self.probation.capacity() == 0 {
            return false;
        }
        if self.probation.len() == PROBATION_ENTRIES {
            self.probation.remove(0);
        }
        self.probation.push(hash);
        false
    }
    fn insert(&mut self, key: Key, shader: Shader, bounds: Rect, pixels: usize, budget: usize) {
        if !self.repeated(key.hash, budget) {
            return;
        }
        let Some(bytes) = pixels
            .checked_add(key.path.len())
            .and_then(|n| n.checked_add(ENTRY_ALLOWANCE))
        else {
            return;
        };
        let container = CONTAINER_ALLOWANCE
            + MAX_ENTRIES * std::mem::size_of::<Entry>()
            + self.probation.capacity() * std::mem::size_of::<u64>();
        if bytes > budget.saturating_sub(container) {
            return;
        }
        if self.entries.capacity() == 0 && self.entries.try_reserve_exact(MAX_ENTRIES).is_err() {
            return;
        }
        self.trim(budget.saturating_sub(bytes));
        if self.entries.capacity() == 0 || self.bytes() > budget.saturating_sub(bytes) {
            return;
        }
        if self.entries.len() == MAX_ENTRIES {
            self.payload_bytes -= self.entries.remove(0).bytes;
        }
        self.entries.push(Entry {
            key,
            shader,
            bounds,
            bytes,
        });
        self.payload_bytes += bytes;
    }
}
thread_local! {static CACHE:RefCell<Cache>=RefCell::new(Cache::default());}
#[cfg(test)]
pub(super) fn retained_bytes() -> usize {
    CACHE.with(|c| c.borrow().bytes())
}
pub(super) fn trim(budget: usize) {
    CACHE.with(|c| c.borrow_mut().trim(budget));
}
pub(super) fn lookup(key: &Key, budget: usize) -> Option<(Shader, Rect)> {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.trim(budget);
        c.find(key)
    })
}
pub(super) fn insert(key: Key, shader: Shader, bounds: Rect, pixels: usize, budget: usize) {
    CACHE.with(|c| c.borrow_mut().insert(key, shader, bounds, pixels, budget));
}

#[cfg(test)]
mod tests {
    use super::*;
    use skia_safe::{Color, PathBuilder, PathFillType};
    fn path(x: f32, fill: PathFillType) -> Path {
        let mut p = PathBuilder::new_with_fill_type(fill);
        p.move_to((0., 0.));
        p.quad_to((x, 1.), (2., 0.));
        p.close();
        p.detach()
    }
    fn key() -> Key {
        Key::new(
            &path(1., PathFillType::Winding),
            &Matrix::default(),
            IRect::from_wh(32, 32),
            2,
        )
    }
    #[test]
    fn one_off_keys_keep_only_bounded_probation_metadata() {
        let mut cache = Cache::default();
        let budget = 8 * 1024 * 1024;
        for x in 0..256 {
            let k = Key::new(
                &path(1., PathFillType::Winding),
                &Matrix::translate((x as f32, 0.)),
                IRect::from_wh(32, 32),
                2,
            );
            cache.insert(
                k,
                skia_safe::shaders::color(Color::WHITE),
                Rect::from_wh(32., 32.),
                2048,
                budget,
            );
            assert!(cache.entries.is_empty());
            assert!(cache.bytes() <= CONTAINER_ALLOWANCE + PROBATION_ENTRIES * 8);
        }
        let p = path(1., PathFillType::Winding);
        let m = Matrix::translate((1000., 0.));
        for _ in 0..2 {
            cache.insert(
                Key::new(&p, &m, IRect::from_wh(32, 32), 2),
                skia_safe::shaders::color(Color::WHITE),
                Rect::from_wh(32., 32.),
                2048,
                budget,
            );
        }
        assert_eq!(cache.entries.len(), 1);
    }
    #[test]
    fn mask_key_checks_every_exact_input_even_under_a_hash_collision() {
        let base = key();
        let mut other = key();
        assert!(base.matches(&other));
        other.matrix[2] = f32::from_bits(1).to_bits();
        other.hash = base.hash;
        assert!(!base.matches(&other));
        other = key();
        other.clip[2] -= 1;
        other.hash = base.hash;
        assert!(!base.matches(&other));
        other = key();
        other.grid = 1;
        other.hash = base.hash;
        assert!(!base.matches(&other));
        other = Key::new(
            &path(f32::from_bits(1f32.to_bits() + 1), PathFillType::Winding),
            &Matrix::default(),
            IRect::from_wh(32, 32),
            2,
        );
        other.hash = base.hash;
        assert!(!base.matches(&other));
        other = Key::new(
            &path(1., PathFillType::EvenOdd),
            &Matrix::default(),
            IRect::from_wh(32, 32),
            2,
        );
        other.hash = base.hash;
        assert!(!base.matches(&other));
    }
    #[test]
    fn mask_cache_charges_pixels_keys_capacity_and_evicts_exact_lru() {
        let mut cache = Cache::default();
        let unit = key().path.len() + 2048 + ENTRY_ALLOWANCE;
        let budget = CONTAINER_ALLOWANCE
            + MAX_ENTRIES * std::mem::size_of::<Entry>()
            + PROBATION_ENTRIES * 8
            + 2 * unit;
        for n in 1..=2 {
            let mut k = key();
            k.grid = n;
            cache.repeated(k.hash, budget);
            cache.insert(
                k,
                skia_safe::shaders::color(Color::WHITE),
                Rect::from_wh(32., 32.),
                2048,
                budget,
            );
        }
        let mut first = key();
        first.grid = 1;
        assert!(cache.find(&first).is_some());
        let mut third = key();
        third.grid = 0;
        cache.repeated(third.hash, budget);
        cache.insert(
            third,
            skia_safe::shaders::color(Color::BLACK),
            Rect::from_wh(32., 32.),
            2048,
            budget,
        );
        assert!(cache.find(&key()).is_none());
        assert!(cache.find(&first).is_some());
        assert!(cache.bytes() <= budget);
        let retained = cache.find(&first).unwrap().0;
        cache.trim(0);
        assert_eq!(cache.bytes(), 0);
        drop(retained); // active draw handle has separate working-memory lifetime.
    }
}
