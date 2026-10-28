use glyphfolio_editor_core::ByteOffset;
use glyphfolio_editor_layout::{
    FontFamily, FontStyle, LayoutOptions, ShapingEngine, TextStyleRange,
};
use unicode_segmentation::UnicodeSegmentation;

#[test]
fn font_runs_preserve_source_coordinates_unicode_and_empty_lines() {
    let text = "a e\u{301}\n\n日本 العربية\n";
    let mut engine = ShapingEngine::new();
    let styles = [
        TextStyleRange {
            range: ByteOffset(0)..ByteOffset(1),
            style: FontStyle {
                weight: 700,
                ..Default::default()
            },
        },
        TextStyleRange {
            range: ByteOffset(2)..ByteOffset(5),
            style: FontStyle {
                italic: true,
                ..Default::default()
            },
        },
        TextStyleRange {
            range: ByteOffset(7)..ByteOffset(13),
            style: FontStyle {
                family: FontFamily::Monospace,
                ..Default::default()
            },
        },
    ];
    let document = engine
        .shape_styled(
            text,
            LayoutOptions::default(),
            FontStyle::default(),
            &styles,
        )
        .unwrap();
    assert_eq!(document.text, text);
    assert_eq!(
        document
            .rows
            .iter()
            .map(|row| row.logical_line)
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    let legal: Vec<_> = text
        .grapheme_indices(true)
        .map(|(at, _)| at)
        .chain([text.len()])
        .collect();
    for row in &document.rows {
        for caret in &row.carets {
            assert!(legal.contains(&caret.byte.0));
        }
    }
    for glyph in &document.glyphs {
        assert!(text.is_char_boundary(glyph.source_range.start.0));
        assert!(text.is_char_boundary(glyph.source_range.end.0));
    }
    let heading = engine.shape_with_style(
        "heading",
        LayoutOptions {
            font_size: 38.0,
            line_height: 50.0,
            ..Default::default()
        },
        FontStyle {
            weight: 700,
            ..Default::default()
        },
    );
    assert_eq!(heading.height, 50.0);
    assert_eq!(heading.text, "heading");
}

#[test]
fn invalid_font_runs_are_refused_before_slicing() {
    let mut engine = ShapingEngine::new();
    let range = |a, b| TextStyleRange {
        range: ByteOffset(a)..ByteOffset(b),
        style: FontStyle::default(),
    };
    for spans in [
        vec![range(1, 2)],
        vec![range(0, 3), range(2, 4)],
        vec![range(0, 9)],
        vec![range(3, 4), range(0, 1)],
    ] {
        assert!(
            engine
                .shape_styled(
                    "éabc",
                    LayoutOptions::default(),
                    FontStyle::default(),
                    &spans
                )
                .is_err()
        );
    }
}

#[test]
fn retained_raster_cache_is_bounded_across_many_font_sizes() {
    let mut engine = ShapingEngine::new();
    if engine.font_faces() == 0 {
        return;
    }
    let mut before = 0;
    let mut observed_reset = false;
    for size in 10..110 {
        let _ = engine.shape(
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
            LayoutOptions {
                font_size: size as f32,
                line_height: size as f32 * 1.4,
                width: 100_000.0,
                ..Default::default()
            },
        );
        let stats = engine.raster_cache_stats();
        assert!(stats.image_entries <= 4096);
        assert!(stats.rgba_bytes <= 32 * 1024 * 1024);
        assert!(stats.queries_since_reset <= 8192);
        observed_reset |= stats.image_entries < before;
        before = stats.image_entries;
    }
    assert!(
        observed_reset,
        "fixture must exceed at least one retained-cache budget"
    );
}

#[test]
fn styled_multiline_document_only_visits_intersecting_font_runs() {
    let mut text = String::new();
    let mut spans = Vec::new();
    for _ in 0..10_000 {
        let start = text.len();
        text.push_str("bold code\n");
        spans.push(TextStyleRange {
            range: ByteOffset(start)..ByteOffset(start + 4),
            style: FontStyle {
                weight: 700,
                ..Default::default()
            },
        });
        spans.push(TextStyleRange {
            range: ByteOffset(start + 5)..ByteOffset(start + 9),
            style: FontStyle {
                family: FontFamily::Monospace,
                ..Default::default()
            },
        });
    }
    let mut engine = ShapingEngine::new();
    let document = engine
        .shape_styled(
            &text,
            LayoutOptions::default(),
            FontStyle::default(),
            &spans,
        )
        .unwrap();
    assert_eq!(document.rows.len(), 10_001);
    assert_eq!(document.diagnostics.style_runs_visited, 20_000);
    assert_eq!(document.text, text);
}
