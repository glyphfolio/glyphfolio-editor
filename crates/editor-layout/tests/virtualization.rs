use std::sync::Arc;

use glyphfolio_editor_core::{Affinity, ByteOffset};
use glyphfolio_editor_layout::{
    BlockStyle, FontFamily, FontStyle, HeightIndex, LayoutBlock, LayoutBlockId, LayoutConfig,
    LayoutError, LongBlockPolicy, Point, ShapingEngine, SourceDocumentBlocks, Viewport,
    VirtualDocument,
};

#[test]
fn height_index_agrees_with_linear_prefix_and_y_model_after_ten_thousand_updates() {
    let mut reference: Vec<f32> = (0..10_000).map(|i| 12.0 + (i % 29) as f32).collect();
    let mut index = HeightIndex::new(reference.clone()).unwrap();
    let mut seed = 41u64;
    for _ in 0..10_000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let at = seed as usize % reference.len();
        let height = 9.0 + ((seed >> 32) % 53) as f32;
        reference[at] = height;
        index.update(at, height).unwrap();
        let expected: f32 = reference[..at].iter().sum();
        assert_eq!(index.prefix(at), expected);
        assert_eq!(index.block_at_y(expected + height / 2.0), Some(at));
    }
    assert_eq!(index.total(), reference.iter().sum());
    assert_eq!(index.block_at_y(-5.0), Some(0));
    assert_eq!(index.block_at_y(index.total() + 1.0), Some(9999));
    let prior = index.total();
    assert!(index.update(0, f32::NAN).is_err());
    assert_eq!(index.total(), prior);
    assert_eq!(HeightIndex::default().block_at_y(0.0), None);
}

#[test]
fn source_line_identity_survives_prefix_edits_duplicates_and_one_line_replacement() {
    let mut lines = SourceDocumentBlocks::default();
    let first = lines.update(Arc::from("a\r\nb\r\nb\r\n\n"), BlockStyle::default());
    assert_eq!(first.len(), 5);
    let prefixed = lines.update(Arc::from("new\na\r\nb\r\nb\r\n\n"), BlockStyle::default());
    assert_eq!(
        first.iter().map(|b| b.id).collect::<Vec<_>>(),
        prefixed[1..].iter().map(|b| b.id).collect::<Vec<_>>()
    );
    let replaced = lines.update(
        Arc::from("new\na\r\n日本\r\nb\r\n\n"),
        BlockStyle::default(),
    );
    assert_eq!(prefixed[2].id, replaced[2].id);
    assert_ne!(prefixed[2].generation, replaced[2].generation);
    assert_eq!(
        replaced.iter().map(|b| b.content()).collect::<String>(),
        "new\na\r\n日本\r\nb\r\n\n"
    );
}

#[test]
fn ten_thousand_lines_shape_only_the_viewport_and_reuse_unchanged_cache() {
    let source = (0..10_000)
        .map(|i| format!("line {i} e\u{301} 日本\n"))
        .collect::<String>();
    let mut lines = SourceDocumentBlocks::default();
    let blocks = lines.update(Arc::from(source.clone()), BlockStyle::default());
    let config = LayoutConfig {
        width: 600.0,
        max_cached_blocks: 32,
        max_blocks_per_pass: 32,
        max_cached_bytes: 4 * 1024 * 1024,
        ..Default::default()
    };
    let mut document = VirtualDocument::new(config).unwrap();
    document.set_blocks(blocks, config).unwrap();
    let mut engine = ShapingEngine::new();
    let first = document.layout_viewport(
        &mut engine,
        Viewport {
            top: 0.0,
            height: 280.0,
            overscan: 28.0,
        },
    );
    assert_eq!(first.stats.total_blocks, 10_001);
    assert!(first.stats.shaped_blocks > 0 && first.stats.shaped_blocks <= 16);
    assert!(first.unmeasured.is_empty());
    assert!(!first.height_is_exact);
    assert!(first.blocks.iter().all(|b| b.index < 20));
    let again = document.layout_viewport(
        &mut engine,
        Viewport {
            top: 0.0,
            height: 280.0,
            overscan: 28.0,
        },
    );
    assert_eq!(again.stats.shaped_blocks, 0);
    assert_eq!(again.stats.cache_hits, again.blocks.len());
    assert!(again.stats.cache_entries <= 32 && again.stats.cache_bytes <= config.max_cached_bytes);
    let remote = document.layout_viewport(
        &mut engine,
        Viewport {
            top: 140_000.0,
            height: 280.0,
            overscan: 28.0,
        },
    );
    assert!(remote.stats.shaped_blocks <= 16);
    assert!(
        remote
            .blocks
            .iter()
            .all(|b| b.index > 4900 && b.index < 5100)
    );
    assert!(
        remote.stats.cache_entries <= 32 && remote.stats.cache_bytes <= config.max_cached_bytes
    );
    assert!(remote.stats.evicted_blocks == 0 || remote.stats.cache_entries == 32);
    let caret = remote
        .hit_test(Point {
            x: 0.0,
            y: remote.corrected_scroll_top + 5.0,
        })
        .unwrap();
    assert!(source.is_char_boundary(caret.byte.0));
    let located = remote.caret(caret.byte, caret.affinity).unwrap();
    assert!((located.x - caret.x).abs() < 0.1 && (located.y - caret.y).abs() < 0.1);
}

#[test]
fn line_endings_do_not_double_height_and_composite_offsets_remain_global() {
    let text = "one\r\ntwo\n";
    let mut source = SourceDocumentBlocks::default();
    let config = LayoutConfig::default();
    let mut virtual_document = VirtualDocument::new(config).unwrap();
    virtual_document
        .set_blocks(
            source.update(Arc::from(text), BlockStyle::default()),
            config,
        )
        .unwrap();
    let mut engine = ShapingEngine::new();
    let view = virtual_document.layout_viewport(&mut engine, Viewport::default());
    assert_eq!(view.blocks.len(), 3);
    assert_eq!(
        view.blocks
            .iter()
            .map(|b| b.shaped.rows.len())
            .sum::<usize>(),
        3
    );
    assert!((view.total_height - 84.0).abs() < 0.1);
    let composite = view.to_shaped_document(text.into());
    assert_eq!(
        composite
            .rows
            .iter()
            .map(|r| r.logical_line)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(composite.rows[1].source_range.start, ByteOffset(5));
    assert_eq!(
        composite.rows.last().unwrap().source_range.start,
        ByteOffset(text.len())
    );
    assert_eq!(
        view.caret(ByteOffset(5), Affinity::After).unwrap().byte,
        ByteOffset(5)
    );
    assert!(view.selection_rects(ByteOffset(1)..ByteOffset(7)).len() >= 2);
}

#[test]
fn cache_budget_offscreen_resolution_and_navigation_do_not_guess_across_gaps() {
    let text = (0..400).map(|i| format!("line {i}\n")).collect::<String>();
    let mut source = SourceDocumentBlocks::default();
    let config = LayoutConfig {
        max_cached_blocks: 5,
        max_blocks_per_pass: 16,
        ..Default::default()
    };
    let mut document = VirtualDocument::new(config).unwrap();
    document
        .set_blocks(
            source.update(Arc::from(text.clone()), BlockStyle::default()),
            config,
        )
        .unwrap();
    let mut engine = ShapingEngine::new();
    let first = document.layout_viewport(
        &mut engine,
        Viewport {
            height: 280.0,
            ..Default::default()
        },
    );
    assert!(first.blocks.len() > 5);
    assert!(first.stats.cache_entries <= 5);
    assert!(
        first
            .caret(ByteOffset(text.len()), Affinity::After)
            .is_none()
    );
    let end = document
        .layout_around_display(&mut engine, ByteOffset(text.len()), Affinity::After)
        .unwrap();
    assert_eq!(
        end.caret(ByteOffset(text.len()), Affinity::After)
            .unwrap()
            .byte,
        ByteOffset(text.len())
    );
    assert!(end.blocks.iter().all(|b| b.index >= 399));
    let mut discontinuous = first;
    discontinuous.blocks.extend(end.blocks);
    discontinuous.blocks.sort_by_key(|b| b.index);
    let last_first_region = discontinuous
        .blocks
        .iter()
        .rfind(|b| b.index < 399)
        .unwrap();
    let last = last_first_region
        .shaped
        .rows
        .last()
        .unwrap()
        .carets
        .last()
        .unwrap();
    let global = last_first_region.global_caret(*last);
    assert!(
        discontinuous
            .vertical_target(global.byte, global.affinity, true, global.x)
            .is_none()
    );
}

#[test]
fn scroll_anchor_tracks_content_through_prefix_insertion_and_width_reflow() {
    let text = (0..100)
        .map(|i| format!("paragraph {i} has several words and wraps across narrow widths\n"))
        .collect::<String>();
    let mut source = SourceDocumentBlocks::default();
    let wide = LayoutConfig {
        width: 700.0,
        ..Default::default()
    };
    let mut document = VirtualDocument::new(wide).unwrap();
    document
        .set_blocks(
            source.update(Arc::from(text.clone()), BlockStyle::default()),
            wide,
        )
        .unwrap();
    let mut engine = ShapingEngine::new();
    let caret_byte = text.find("paragraph 50").unwrap();
    let exact = document
        .ensure_display_byte(&mut engine, ByteOffset(caret_byte), Affinity::After)
        .unwrap();
    let top = exact.text_origin.y + 3.0;
    let anchor = document.capture_anchor(top).unwrap();
    let changed = format!("inserted\n{text}");
    document
        .set_blocks(
            source.update(Arc::from(changed), BlockStyle::default()),
            wide,
        )
        .unwrap();
    let after_prefix = document.resolve_anchor(&mut engine, anchor).unwrap();
    assert!((after_prefix - top - 28.0).abs() < 0.1);
    let narrower = LayoutConfig {
        width: 120.0,
        ..wide
    };
    document.configure(narrower).unwrap();
    let after_width = document.resolve_anchor(&mut engine, anchor).unwrap();
    let index = document
        .block_for_display(ByteOffset(caret_byte + 9), Affinity::After)
        .unwrap();
    let block = document.ensure_block_at(&mut engine, index).unwrap();
    let caret = block.global_caret(
        block
            .shaped
            .caret(anchor.local_display_byte, Affinity::After),
    );
    assert!((after_width - caret.y - anchor.offset_y).abs() < 0.1);
}

#[test]
fn giant_paragraph_is_explicitly_deferred_and_exact_work_is_opt_in() {
    let text: Arc<str> = Arc::from("word ".repeat(20_000));
    let block = LayoutBlock::from_shared(
        LayoutBlockId(7),
        1,
        ByteOffset(0)..ByteOffset(text.len()),
        ByteOffset(0)..ByteOffset(text.len()),
        text,
        BlockStyle::default(),
    );
    let mut document = VirtualDocument::default();
    document
        .set_blocks(vec![block], LayoutConfig::default())
        .unwrap();
    let mut engine = ShapingEngine::new();
    let view = document.layout_viewport(&mut engine, Viewport::default());
    assert!(view.blocks.is_empty());
    assert_eq!(view.stats.shaped_blocks, 0);
    assert_eq!(view.stats.deferred_long_blocks, 1);
    assert!(!view.height_is_exact);
    assert!(matches!(
        document.ensure_display_byte(&mut engine, ByteOffset(0), Affinity::After),
        Err(LayoutError::DeferredLongBlock)
    ));
    let smaller: Arc<str> = Arc::from("word ".repeat(100));
    let exact = LayoutConfig {
        long_block_bytes: 64,
        long_block_policy: LongBlockPolicy::Exact,
        ..Default::default()
    };
    document
        .set_blocks(
            vec![LayoutBlock::from_shared(
                LayoutBlockId(7),
                2,
                ByteOffset(0)..ByteOffset(smaller.len()),
                ByteOffset(0)..ByteOffset(smaller.len()),
                smaller,
                BlockStyle::default(),
            )],
            exact,
        )
        .unwrap();
    let view = document.layout_viewport(&mut engine, Viewport::default());
    assert_eq!(view.stats.exact_long_blocks, 1);
    assert_eq!(view.stats.largest_shaped_block_bytes, 500);
    assert!(view.height_is_exact);
}

#[test]
fn invalid_blocks_are_atomic_and_typography_invalidates_measurement() {
    let mut document = VirtualDocument::default();
    let mut lines = SourceDocumentBlocks::default();
    let blocks = lines.update(Arc::from("日本\nnext"), BlockStyle::default());
    document
        .set_blocks(blocks.clone(), LayoutConfig::default())
        .unwrap();
    let mut engine = ShapingEngine::new();
    let view = document.layout_viewport(&mut engine, Viewport::default());
    let old_height = view.total_height;
    let mut bad = blocks.clone();
    bad[0].text_range.start = 1;
    assert!(document.set_blocks(bad, LayoutConfig::default()).is_err());
    assert_eq!(document.block_count(), 2);
    assert_eq!(document.heights().total(), old_height);
    let mut changed = blocks;
    changed[0].style = BlockStyle {
        font: FontStyle {
            family: FontFamily::Monospace,
            weight: 700,
            italic: false,
        },
        font_size: 38.0,
        line_height: 50.0,
        ..Default::default()
    };
    document
        .set_blocks(changed, LayoutConfig::default())
        .unwrap();
    let view = document.layout_viewport(&mut engine, Viewport::default());
    assert_eq!(view.stats.shaped_blocks, 1);
    assert!(view.total_height > old_height);
}
