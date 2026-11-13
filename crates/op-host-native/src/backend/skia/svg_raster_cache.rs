//! Byte-bounded LRU for the optional complex-path raster acceleration.
//!
//! The entry limit alone permits almost 2 GiB of N32 pixels. Keep a small
//! working set instead; eviction changes reuse, never raster resolution.

use std::collections::HashMap;

use op_editor_ui::Point2D;

const BYTE_BUDGET: usize = 32 * 1024 * 1024;
const MAX_ENTRIES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SvgRasterKey {
    pub path_key: u64,
    pub d_len: usize,
    pub even_odd: bool,
    pub color_rgba: u32,
    pub size_bits: u32,
    pub viewbox_bits: u32,
    pub dpi_bits: u32,
}

pub(super) struct SvgRasterCacheEntry {
    pub image: skia_safe::Image,
    pub offset: Point2D,
    pub size: Point2D,
}

struct CachedRaster {
    raster: SvgRasterCacheEntry,
    bytes: usize,
    last_used: u64,
}

pub(super) struct SvgRasterCache {
    entries: HashMap<SvgRasterKey, CachedRaster>,
    bytes: usize,
    tick: u64,
    byte_budget: usize,
    max_entries: usize,
}

impl Default for SvgRasterCache {
    fn default() -> Self {
        Self::with_limits(BYTE_BUDGET, MAX_ENTRIES)
    }
}

impl SvgRasterCache {
    fn with_limits(byte_budget: usize, max_entries: usize) -> Self {
        Self {
            entries: HashMap::new(),
            bytes: 0,
            tick: 0,
            byte_budget,
            max_entries,
        }
    }

    pub fn contains_key(&self, key: &SvgRasterKey) -> bool {
        self.entries.contains_key(key)
    }

    pub fn get(&mut self, key: &SvgRasterKey) -> Option<&SvgRasterCacheEntry> {
        let entry = self.entries.get_mut(key)?;
        self.tick = self.tick.wrapping_add(1);
        entry.last_used = self.tick;
        Some(&entry.raster)
    }

    pub fn insert(&mut self, key: SvgRasterKey, raster: SvgRasterCacheEntry) {
        // These are raster_n32 snapshots. Use the actual row stride when
        // available so row padding also counts against the pixel budget.
        let bytes = raster.image.peek_pixels().map_or_else(
            || raster.image.image_info().compute_min_byte_size(),
            |pixels| {
                pixels
                    .row_bytes()
                    .saturating_mul(pixels.height().max(0) as usize)
            },
        );
        if bytes > self.byte_budget || self.max_entries == 0 {
            return;
        }
        if let Some(old) = self.entries.remove(&key) {
            self.bytes = self.bytes.saturating_sub(old.bytes);
        }
        // Evict before installing, keeping retained cache pixels within the
        // limit even while the caller temporarily owns its newly drawn image.
        while self.bytes.saturating_add(bytes) > self.byte_budget
            || self.entries.len() >= self.max_entries
        {
            let Some((&oldest, _)) = self.entries.iter().min_by_key(|(_, e)| e.last_used) else {
                break;
            };
            if let Some(old) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(old.bytes);
            }
        }
        self.tick = self.tick.wrapping_add(1);
        self.bytes = self.bytes.saturating_add(bytes);
        self.entries.insert(
            key,
            CachedRaster {
                raster,
                bytes,
                last_used: self.tick,
            },
        );
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
#[path = "svg_raster_cache_tests.rs"]
mod tests;
