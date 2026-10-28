use glyphfolio_editor_core::{Affinity, ByteOffset};
use glyphfolio_editor_layout::{LayoutOptions, Point, ShapingEngine};
use unicode_segmentation::UnicodeSegmentation;

#[test]
fn source_bytes_endings_empty_rows_and_tab_positions_remain_exact() {
    let mut engine = ShapingEngine::new();
    let text = "\u{feff}a\té\r\n\r\n日本\rfinal\n";
    let document = engine.shape(text, LayoutOptions::default());
    assert_eq!(document.text, text);
    assert_eq!(
        document.rows.iter().map(|row| row.logical_line).max(),
        Some(4)
    );
    assert_eq!(
        document.rows.last().unwrap().source_range,
        ByteOffset(text.len())..ByteOffset(text.len())
    );
    for row in &document.rows {
        assert!(text.is_char_boundary(row.source_range.start.0));
        assert!(text.is_char_boundary(row.source_range.end.0));
        assert!(!row.carets.is_empty());
        for caret in &row.carets {
            assert!(text.is_char_boundary(caret.byte.0));
        }
    }
    let empty = engine.shape("", LayoutOptions::default());
    assert_eq!(empty.rows.len(), 1);
    assert_eq!(empty.hit_test(Point { x: 0.0, y: 0.0 }).byte, ByteOffset(0));
}

#[test]
fn wraps_and_clusters_expose_only_legal_carets_and_matching_hit_geometry() {
    let mut engine = ShapingEngine::new();
    let text = "office e\u{301} 👩‍👩‍👧‍👦\nLatin text with several wrapped lines and 日本語";
    let document = engine.shape(
        text,
        LayoutOptions {
            width: 150.0,
            ..Default::default()
        },
    );
    assert!(document.rows.len() > 2);
    let legal: Vec<_> = text
        .grapheme_indices(true)
        .map(|(byte, _)| byte)
        .chain([text.len()])
        .collect();
    for row in &document.rows {
        for caret in &row.carets {
            assert!(
                legal.contains(&caret.byte.0),
                "illegal caret {}",
                caret.byte.0
            );
            let hit = document.hit_test(Point {
                x: caret.x,
                y: row.bounds.center().y,
            });
            assert!(legal.contains(&hit.byte.0));
            assert!((hit.x - caret.x).abs() < 0.1);
            let rect = document.caret_rect(caret.byte, caret.affinity);
            assert!(rect.x.is_finite() && rect.y.is_finite());
        }
    }
    assert!(
        document
            .selection_rects(ByteOffset(3)..ByteOffset(3))
            .is_empty()
    );
}

#[test]
fn complex_script_cluster_splits_never_create_interior_grapheme_carets() {
    let mut engine = ShapingEngine::new();
    for text in [
        "नमस्ते",
        "العَرَبِيَّة",
        "a\u{301}\u{308}",
        "👩‍👩‍👧‍👦",
        "אבג abc",
        "\0\u{feff}e\u{301}",
    ] {
        let document = engine.shape(text, LayoutOptions::default());
        let legal: Vec<_> = text
            .grapheme_indices(true)
            .map(|(byte, _)| byte)
            .chain([text.len()])
            .collect();
        for row in &document.rows {
            for caret in &row.carets {
                assert!(
                    legal.contains(&caret.byte.0),
                    "interior grapheme caret {} in {text:?}",
                    caret.byte.0
                );
            }
        }
    }
    let invalid = engine.shape(
        "safe",
        LayoutOptions {
            width: f32::NAN,
            font_size: f32::INFINITY,
            line_height: -1.0,
            scale: f32::NAN,
        },
    );
    assert!(invalid.height.is_finite());
}

#[test]
fn partial_shaping_clusters_keep_grapheme_end_carets_at_full_advance_extents() {
    let mut engine = ShapingEngine::new();
    // One EGC can be split into several font/shaping clusters. On the native
    // Devanagari fallback the last cluster begins inside this 12-byte EGC; its
    // only legal stop is its end, which must map to the full cluster advance.
    for text in ["क्‍ष", "क्‍षक्‍ष", "aक्‍षz"] {
        let document = engine.shape(text, LayoutOptions::default());
        let row = &document.rows[0];
        let legal: Vec<_> = text
            .grapheme_indices(true)
            .map(|(byte, _)| byte)
            .chain([text.len()])
            .collect();
        for cluster in &row.clusters {
            for (byte, affinity, expected_x) in [
                (
                    cluster.source_range.start,
                    Affinity::After,
                    if cluster.rtl {
                        cluster.bounds.right()
                    } else {
                        cluster.bounds.x
                    },
                ),
                (
                    cluster.source_range.end,
                    Affinity::Before,
                    if cluster.rtl {
                        cluster.bounds.x
                    } else {
                        cluster.bounds.right()
                    },
                ),
            ] {
                if legal.contains(&byte.0) {
                    assert!(
                        row.carets.iter().any(|caret| caret.byte == byte
                            && caret.affinity == affinity
                            && (caret.x - expected_x).abs() < 0.01),
                        "legal source edge {byte:?}/{affinity:?} does not match shaping-cluster edge {expected_x} for {text:?}"
                    );
                }
            }
        }
        let left = row
            .clusters
            .iter()
            .map(|cluster| cluster.bounds.x)
            .fold(f32::INFINITY, f32::min);
        let right = row
            .clusters
            .iter()
            .map(|cluster| cluster.bounds.right())
            .fold(f32::NEG_INFINITY, f32::max);
        assert!((document.caret(ByteOffset(0), Affinity::After).x - left).abs() < 0.01);
        let end = document.caret(ByteOffset(text.len()), Affinity::Before);
        assert!(
            (end.x - right).abs() < 0.01,
            "end caret must follow the entire shaped advance for {text:?}"
        );
        assert_eq!(
            document
                .hit_test(Point {
                    x: right,
                    y: row.bounds.center().y
                })
                .byte,
            ByteOffset(text.len())
        );
        let selected = document.selection_rects(ByteOffset(0)..ByteOffset(text.len()));
        assert!((selected.first().unwrap().x - left).abs() < 0.01);
        assert!((selected.last().unwrap().right() - right).abs() < 0.01);
    }
}

#[test]
fn rtl_clusters_reverse_source_order_but_keep_logical_selections() {
    let mut engine = ShapingEngine::new();
    let text = "abc אבג xyz";
    let document = engine.shape(text, LayoutOptions::default());
    let mut rtl: Vec<_> = document.rows[0]
        .clusters
        .iter()
        .filter(|cluster| cluster.rtl)
        .collect();
    rtl.sort_by(|a, b| a.bounds.x.total_cmp(&b.bounds.x));
    assert!(rtl.len() >= 3);
    assert!(
        rtl.windows(2)
            .any(|pair| pair[0].source_range.start > pair[1].source_range.start)
    );
    let selected = document.selection_rects(ByteOffset(0)..ByteOffset(6));
    assert!(
        selected.len() >= 2,
        "partial RTL logical selection must produce disjoint visual rectangles: {selected:?}"
    );
    let caret = document.caret(ByteOffset(4), Affinity::After);
    let moved = document.visual_neighbor(caret.byte, caret.affinity, true);
    assert!(moved.x > caret.x || moved.y > caret.y);
}

#[test]
fn raster_masks_and_color_images_have_complete_rgba_and_scale_consistent_bounds() {
    let mut engine = ShapingEngine::new();
    let one = engine.shape("Test é", LayoutOptions::default());
    let two = engine.shape(
        "Test é",
        LayoutOptions {
            scale: 2.0,
            ..Default::default()
        },
    );
    assert!(!one.glyphs.is_empty());
    for glyph in one.glyphs.iter().chain(&two.glyphs) {
        assert_eq!(
            glyph.image.rgba.len(),
            glyph.image.width as usize * glyph.image.height as usize * 4
        );
        assert!(glyph.bounds.width > 0.0 && glyph.bounds.height > 0.0);
    }
    assert!((one.rows[0].bounds.width - two.rows[0].bounds.width).abs() < 0.1);
}

#[cfg(target_os = "macos")]
#[test]
fn native_macos_font_coverage_and_composed_color_emoji() {
    let mut engine = ShapingEngine::new();
    assert!(
        engine.font_faces() > 0,
        "F2 requires installed system fonts"
    );
    for (label, text) in [
        ("Latin", "office affinity café e\u{301}"),
        ("Japanese", "日本語を入力"),
        ("Chinese", "中文输入"),
        ("Indic", "नमस्ते"),
        ("Arabic", "العربية"),
        ("Hebrew", "עברית"),
    ] {
        let document = engine.shape(
            text,
            LayoutOptions {
                scale: 2.0,
                ..Default::default()
            },
        );
        println!("F2 {label}: {:?}", document.diagnostics);
        assert_eq!(document.diagnostics.missing_glyphs, 0, "{label}");
        assert_eq!(document.diagnostics.raster_failures, 0, "{label}");
        assert!(!document.glyphs.is_empty(), "{label}");
    }
    let family = engine.shape(
        "👩‍👩‍👧‍👦",
        LayoutOptions {
            scale: 2.0,
            ..Default::default()
        },
    );
    println!("F2 family emoji: {:?}", family.diagnostics);
    assert_eq!(family.diagnostics.missing_glyphs, 0);
    assert_eq!(family.diagnostics.raster_failures, 0);
    assert_eq!(
        family.diagnostics.glyph_count, 1,
        "ZWJ family must shape as one visible glyph"
    );
    assert_eq!(
        family.diagnostics.color_glyphs, 1,
        "native color emoji raster"
    );
}
