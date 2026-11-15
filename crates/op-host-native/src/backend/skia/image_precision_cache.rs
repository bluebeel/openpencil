//! Immutable source-image F16 conversion reuse, independent of CTM/brush data.
//! Original handles stay alive, so their native unique IDs cannot be reused.
use skia_safe::Image;
use std::{cell::RefCell, mem::size_of};
const MAX_ENTRIES: usize = 8;
const MAX_BYTES: usize = 128 * 1024 * 1024;
struct Entry {
    original: Image,
    converted: Image,
    bytes: usize,
}
#[derive(Default)]
struct Cache {
    entries: Vec<Entry>,
    payload: usize,
}
impl Cache {
    fn retained(&self) -> usize {
        self.payload + self.entries.capacity() * size_of::<Entry>()
    }
    fn get(&mut self, image: &Image) -> Option<Image> {
        // Skia Image is immutable and unique_id identifies that native object;
        // the retained original is the lifetime proof, not a Doc revision/hash.
        let index = self
            .entries
            .iter()
            .position(|entry| entry.original.unique_id() == image.unique_id())?;
        let entry = self.entries.remove(index);
        let result = entry.converted.clone();
        self.entries.push(entry);
        Some(result)
    }
    fn insert(&mut self, original: &Image, converted: &Image) {
        fn pixels(image: &Image) -> usize {
            image.peek_pixels().map_or_else(
                || image.image_info().compute_min_byte_size(),
                |pixels| {
                    pixels
                        .row_bytes()
                        .saturating_mul(pixels.height().max(0) as usize)
                },
            )
        }
        // Charge both the pinned source and F16 allocation even if another
        // LRU currently shares source pixels. Opaque native overhead is bounded
        // by entry count and a conservative per-entry allowance, not RSS proof.
        let bytes = pixels(original)
            .saturating_add(pixels(converted))
            .saturating_add(4096);
        let container = MAX_ENTRIES * size_of::<Entry>();
        if bytes > MAX_BYTES.saturating_sub(container) {
            return;
        }
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.original.unique_id() == original.unique_id())
        {
            self.payload -= self.entries.remove(index).bytes;
        }
        if self.entries.capacity() == 0 && self.entries.try_reserve_exact(MAX_ENTRIES).is_err() {
            return;
        }
        while !self.entries.is_empty()
            && (self.entries.len() == MAX_ENTRIES
                || self.retained() > MAX_BYTES.saturating_sub(bytes))
        {
            self.payload -= self.entries.remove(0).bytes;
        }
        if self.retained() > MAX_BYTES.saturating_sub(bytes) {
            return;
        }
        self.entries.push(Entry {
            original: original.clone(),
            converted: converted.clone(),
            bytes,
        });
        self.payload += bytes;
    }
}
thread_local! { static CACHE: RefCell<Cache> = RefCell::new(Cache::default()); }
pub(super) fn get(image: &Image) -> Option<Image> {
    CACHE.with(|cache| cache.borrow_mut().get(image))
}
pub(super) fn remember(original: &Image, converted: &Image) {
    CACHE.with(|cache| cache.borrow_mut().insert(original, converted));
}
