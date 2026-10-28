use glyphfolio_editor_core::{Affinity, ByteOffset, ColumnAlignment};
use glyphfolio_editor_layout::{FontStyle, LayoutOptions, Point, ShapingEngine, TableLayoutSpec};
fn spec() -> TableLayoutSpec {
    TableLayoutSpec {
        rows: vec![
            vec![ByteOffset(0)..ByteOffset(4), ByteOffset(5)..ByteOffset(10)],
            vec![
                ByteOffset(11)..ByteOffset(12),
                ByteOffset(13)..ByteOffset(14),
            ],
        ],
        alignments: vec![ColumnAlignment::Left, ColumnAlignment::Right],
    }
}
#[test]
fn native_cells_wrap_align_and_hit_test_without_text_grid_characters() {
    let text = "Name\tValue\nx\ty\n";
    let mut engine = ShapingEngine::new();
    let layout = engine
        .shape_table(
            text,
            LayoutOptions {
                width: 400.0,
                font_size: 16.0,
                line_height: 25.6,
                scale: 1.0,
            },
            FontStyle::default(),
            &[],
            &spec(),
        )
        .unwrap();
    assert_eq!(layout.table_cells.len(), 4);
    assert_eq!(layout.table_cells[1].bounds.x, 200.0);
    let right = layout.caret(ByteOffset(13), Affinity::After);
    let left = layout.caret(ByteOffset(11), Affinity::After);
    assert!(right.x > 300.0 && left.x < 30.0);
    assert_eq!(
        layout
            .hit_test(Point {
                x: right.x,
                y: right.y + 5.0
            })
            .byte,
        ByteOffset(13)
    );
    let down = layout.vertical_target(ByteOffset(5), Affinity::After, true, right.x);
    assert!(down.byte.0 >= 13 && down.byte.0 <= 14);
    let narrow = engine
        .shape_table(
            text,
            LayoutOptions {
                width: 100.0,
                ..LayoutOptions::default()
            },
            FontStyle::default(),
            &[],
            &spec(),
        )
        .unwrap();
    assert_eq!(narrow.width, 192.0);
}
#[test]
fn invalid_table_ranges_are_refused_and_long_cells_wrap_inside_bounds() {
    let mut engine = ShapingEngine::new();
    let mut invalid = spec();
    invalid.rows[0][0] = ByteOffset(1)..ByteOffset(100);
    assert!(
        engine
            .shape_table(
                "short",
                LayoutOptions::default(),
                FontStyle::default(),
                &[],
                &invalid
            )
            .is_err()
    );
    let text = "word ".repeat(60);
    let table = TableLayoutSpec {
        rows: vec![vec![ByteOffset(0)..ByteOffset(text.len())]],
        alignments: vec![ColumnAlignment::Center],
    };
    let layout = engine
        .shape_table(
            &text,
            LayoutOptions {
                width: 160.0,
                ..LayoutOptions::default()
            },
            FontStyle::default(),
            &[],
            &table,
        )
        .unwrap();
    assert!(layout.rows.len() > 3);
    assert!(
        layout
            .glyphs
            .iter()
            .all(|glyph| glyph.bounds.x >= 0.0 && glyph.bounds.right() <= 161.0)
    );
}

#[test]
fn blank_space_in_a_short_cell_and_vertical_navigation_stay_in_its_column() {
    let long = "many words ".repeat(20);
    let text = format!("{long}\tshort\nnext\tsecond\n");
    let short = long.len() + 1;
    let next = short + 6;
    let second = next + 5;
    let table = TableLayoutSpec {
        rows: vec![
            vec![
                ByteOffset(0)..ByteOffset(long.len()),
                ByteOffset(short)..ByteOffset(short + 5),
            ],
            vec![
                ByteOffset(next)..ByteOffset(next + 4),
                ByteOffset(second)..ByteOffset(second + 6),
            ],
        ],
        alignments: vec![ColumnAlignment::Left; 2],
    };
    let layout = ShapingEngine::new()
        .shape_table(
            &text,
            LayoutOptions {
                width: 240.0,
                font_size: 16.0,
                line_height: 25.6,
                scale: 1.0,
            },
            FontStyle::default(),
            &[],
            &table,
        )
        .unwrap();
    let cell = &layout.table_cells[1];
    let hit = layout.hit_test(Point {
        x: cell.bounds.x + 20.0,
        y: cell.bounds.bottom() - 12.0,
    });
    assert!((short..=short + 5).contains(&hit.byte.0));
    let down = layout.vertical_target(
        ByteOffset(short),
        Affinity::After,
        true,
        cell.bounds.x + 20.0,
    );
    assert!((second..=second + 6).contains(&down.byte.0));
}
