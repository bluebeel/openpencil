use super::*;

fn key(index: u64) -> SvgRasterKey {
    SvgRasterKey {
        path_key: index,
        d_len: 4096,
        even_odd: false,
        color_rgba: 0xff123456,
        size_bits: 1.0_f32.to_bits(),
        viewbox_bits: 1.0_f32.to_bits(),
        dpi_bits: 1.0_f32.to_bits(),
    }
}

fn raster(edge: i32) -> SvgRasterCacheEntry {
    let mut surface = skia_safe::surfaces::raster_n32_premul((edge, edge)).unwrap();
    surface
        .canvas()
        .clear(skia_safe::Color::from_rgb(18, 52, 86));
    SvgRasterCacheEntry {
        image: surface.image_snapshot(),
        offset: Point2D::new(-2.0, -2.0),
        size: Point2D::new(edge as f32, edge as f32),
    }
}

#[test]
fn evicts_least_recently_used_raster_and_preserves_reused_pixels() {
    let mut cache = SvgRasterCache::with_limits(2048, 2);
    cache.insert(key(1), raster(16));
    cache.insert(key(2), raster(16));
    let hot = cache.get(&key(1)).unwrap();
    let image_id = hot.image.unique_id();
    let pixels = hot.image.peek_pixels().unwrap().bytes().unwrap().to_vec();
    cache.insert(key(3), raster(16));

    assert!(cache.get(&key(2)).is_none(), "a hit must refresh recency");
    let hot = cache.get(&key(1)).unwrap();
    assert_eq!(hot.image.unique_id(), image_id);
    assert_eq!(hot.image.peek_pixels().unwrap().bytes().unwrap(), pixels);
    assert_eq!(hot.offset, Point2D::new(-2.0, -2.0));
    assert_eq!(hot.size, Point2D::new(16.0, 16.0));
    assert_eq!(cache.bytes, 2048);
}

#[test]
fn replacement_updates_byte_accounting_and_oversize_does_not_flush_hot_entries() {
    let mut cache = SvgRasterCache::with_limits(4096, 8);
    cache.insert(key(1), raster(16));
    cache.insert(key(1), raster(32));
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.bytes, 4096);
    cache.insert(key(2), raster(64));
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.bytes, 4096);
    assert!(cache.get(&key(1)).is_some());
    assert!(cache.get(&key(2)).is_none());
}

#[test]
fn default_cache_stays_within_pixel_budget_during_many_distinct_edits() {
    let mut cache = SvgRasterCache::default();
    for index in 0..256 {
        cache.insert(key(index), raster(512));
        assert!(cache.bytes <= BYTE_BUDGET);
        assert!(cache.len() <= MAX_ENTRIES);
    }
    assert_eq!(cache.bytes, 32 * 1024 * 1024);
    assert_eq!(cache.len(), 32);
    assert!(cache.get(&key(255)).is_some());
    assert!(cache.get(&key(0)).is_none());
}

/// Same workload with either the former count-only capacity (256 MiB for
/// these rasters) or the new default. Run separately under the process guard
/// to measure retained native pixels and process footprint without a fixture.
#[test]
#[ignore = "explicit resource measurement; set OP_SVG_PROBE_BYTE_BUDGET"]
fn svg_raster_cache_memory_probe() {
    let budget = std::env::var("OP_SVG_PROBE_BYTE_BUDGET")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(BYTE_BUDGET);
    let mut cache = SvgRasterCache::with_limits(budget, MAX_ENTRIES);
    let start = std::time::Instant::now();
    for index in 0..256 {
        cache.insert(key(index), raster(512));
    }
    eprintln!(
        "{{\"probe\":\"svg-raster-cache\",\"budgetBytes\":{budget},\"entries\":{},\"retainedPixelBytes\":{},\"elapsedMs\":{}}}",
        cache.len(),
        cache.bytes,
        start.elapsed().as_millis()
    );
    // Let the external 100 ms resource sampler observe the final live set.
    std::thread::sleep(std::time::Duration::from_millis(300));
    std::hint::black_box(cache);
}
