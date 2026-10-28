//! Headless work/latency measurement, not an FPS, GPU, IME, or process-memory gate.
use std::{sync::Arc, time::Instant};

use glyphfolio_editor_core::{Affinity, ByteOffset};
use glyphfolio_editor_layout::{
    BlockStyle, LayoutBlock, LayoutBlockId, LayoutConfig, LongBlockPolicy, ShapingEngine,
    SourceDocumentBlocks, Viewport, VirtualDocument,
};

fn main() {
    let text = (0..10_000)
        .map(|i| format!("line {i} **bold** café 日本\n"))
        .collect::<String>();
    let mut lines = SourceDocumentBlocks::default();
    let config = LayoutConfig {
        width: 800.0,
        max_cached_blocks: 128,
        max_blocks_per_pass: 128,
        ..Default::default()
    };
    let mut virtual_doc = VirtualDocument::new(config).unwrap();
    let build = Instant::now();
    virtual_doc
        .set_blocks(
            lines.update(Arc::from(text.clone()), BlockStyle::default()),
            config,
        )
        .unwrap();
    let build_ms = build.elapsed().as_secs_f64() * 1000.0;
    let fonts = Instant::now();
    let mut engine = ShapingEngine::new();
    let font_ms = fonts.elapsed().as_secs_f64() * 1000.0;
    let visible = Instant::now();
    let first = virtual_doc.layout_viewport(&mut engine, Viewport::default());
    let visible_ms = visible.elapsed().as_secs_f64() * 1000.0;
    let repeat = Instant::now();
    let mut reshaped = 0;
    for _ in 0..20 {
        reshaped += virtual_doc
            .layout_viewport(&mut engine, Viewport::default())
            .stats
            .shaped_blocks;
    }
    let repeat_ms = repeat.elapsed().as_secs_f64() * 1000.0 / 20.0;
    assert_eq!(reshaped, 0);
    let baseline = Instant::now();
    let whole = engine.shape(&text, glyphfolio_editor_layout::LayoutOptions::default());
    let whole_ms = baseline.elapsed().as_secs_f64() * 1000.0;
    let whole_glyphs = whole.diagnostics.glyph_count;
    drop(whole);
    let mut max_blocks = first.stats.cache_entries;
    let mut max_bytes = first.stats.cache_bytes;
    let mut remote_shaped = 0;
    for index in 0..20 {
        let view = virtual_doc.layout_viewport(
            &mut engine,
            Viewport {
                top: index as f32 * 10_000.0,
                ..Default::default()
            },
        );
        remote_shaped += view.stats.shaped_blocks;
        max_blocks = max_blocks.max(view.stats.cache_entries);
        max_bytes = max_bytes.max(view.stats.cache_bytes);
    }
    let middle = text.find("line 5000 ").unwrap();
    let target = virtual_doc
        .ensure_display_byte(&mut engine, ByteOffset(middle), Affinity::After)
        .unwrap();
    let viewport = Viewport {
        top: target.bounds.y,
        ..Default::default()
    };
    let _ = virtual_doc.layout_viewport(&mut engine, viewport);
    let edited = text.replacen("line 5000 ", "line 5000 edited ", 1);
    let edit = Instant::now();
    virtual_doc
        .set_blocks(
            lines.update(Arc::from(edited), BlockStyle::default()),
            config,
        )
        .unwrap();
    let after_edit = virtual_doc.layout_viewport(&mut engine, viewport);
    let edit_ms = edit.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(after_edit.stats.shaped_blocks, 1);
    assert!(max_blocks <= config.max_cached_blocks && max_bytes <= config.max_cached_bytes);
    let large: Arc<str> = Arc::from("word ".repeat(20_000));
    let mut giant = VirtualDocument::default();
    giant
        .set_blocks(
            vec![LayoutBlock::from_shared(
                LayoutBlockId(1),
                1,
                ByteOffset(0)..ByteOffset(large.len()),
                ByteOffset(0)..ByteOffset(large.len()),
                large,
                BlockStyle::default(),
            )],
            LayoutConfig::default(),
        )
        .unwrap();
    let defer = Instant::now();
    let deferred = giant.layout_viewport(&mut engine, Viewport::default());
    let defer_ms = defer.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(deferred.stats.shaped_blocks, 0);
    let exact = Instant::now();
    giant
        .configure(LayoutConfig {
            long_block_policy: LongBlockPolicy::Exact,
            ..Default::default()
        })
        .unwrap();
    let exact_view = giant.layout_viewport(&mut engine, Viewport::default());
    let exact_ms = exact.elapsed().as_secs_f64() * 1000.0;
    let raster = engine.raster_cache_stats();
    println!(
        "{{\"profile\":\"{}\",\"source_bytes\":{},\"logical_blocks\":{},\"font_faces\":{},\"font_init_ms\":{font_ms:.3},\"metadata_build_ms\":{build_ms:.3},\"first_visible_ms\":{visible_ms:.3},\"first_shaped_blocks\":{},\"first_visible_glyphs\":{},\"repeat_visible_mean_ms\":{repeat_ms:.3},\"repeat_reshaped_blocks\":{reshaped},\"full_shape_ms\":{whole_ms:.3},\"full_glyphs\":{whole_glyphs},\"remote_scroll_shaped_total\":{remote_shaped},\"max_cached_blocks\":{max_blocks},\"max_cache_accounted_bytes\":{max_bytes},\"single_line_edit_ms\":{edit_ms:.3},\"single_line_edit_shaped_blocks\":{},\"giant_paragraph_bytes\":100000,\"deferred_frame_ms\":{defer_ms:.3},\"deferred_shaped_blocks\":{},\"exact_giant_ms\":{exact_ms:.3},\"exact_giant_reported\":{},\"raster_entries\":{},\"raster_rgba_bytes\":{}}}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        text.len(),
        first.stats.total_blocks,
        engine.font_faces(),
        first.stats.shaped_blocks,
        first
            .blocks
            .iter()
            .map(|block| block.shaped.diagnostics.glyph_count)
            .sum::<usize>(),
        after_edit.stats.shaped_blocks,
        deferred.stats.shaped_blocks,
        exact_view.stats.exact_long_blocks,
        raster.image_entries,
        raster.rgba_bytes
    );
}
