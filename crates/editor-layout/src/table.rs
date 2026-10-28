//! Original native cell layout over low-level shaping; all ranges are display bytes.
use crate::{
    FontStyle, InvalidStyleRange, LayoutOptions, Rect, ShapedDocument, ShapingEngine,
    TextStyleRange,
};
use glyphfolio_editor_core::{ByteOffset, ColumnAlignment};
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TableLayoutSpec {
    pub rows: Vec<Vec<Range<ByteOffset>>>,
    pub alignments: Vec<ColumnAlignment>,
}
#[derive(Clone, Debug)]
pub struct TableCellGeometry {
    pub range: Range<ByteOffset>,
    pub bounds: Rect,
    pub row: usize,
    pub column: usize,
}
impl TableLayoutSpec {
    pub fn valid(&self, text: &str) -> bool {
        let columns = self.alignments.len();
        let mut previous = None;
        columns > 0
            && !self.rows.is_empty()
            && self.rows.iter().all(|row| {
                row.len() == columns
                    && row.iter().all(|range| {
                        let valid = range.start <= range.end
                            && range.end.0 <= text.len()
                            && text.is_char_boundary(range.start.0)
                            && text.is_char_boundary(range.end.0)
                            && previous.is_none_or(|end: ByteOffset| end < range.start);
                        previous = Some(range.end);
                        valid
                    })
            })
    }
}
impl ShapingEngine {
    pub fn shape_table(
        &mut self,
        text: &str,
        options: LayoutOptions,
        font: FontStyle,
        spans: &[TextStyleRange],
        table: &TableLayoutSpec,
    ) -> Result<ShapedDocument, InvalidStyleRange> {
        if !table.valid(text) {
            return Err(InvalidStyleRange);
        }
        let columns = table.alignments.len();
        let column_width = (options.width / columns as f32).max(96.0);
        let padding = 10.0;
        let mut output = self.shape_styled("", options, font, &[])?;
        output.text = text.into();
        output.rows.clear();
        output.glyphs.clear();
        output.table_cells.clear();
        output.height = 0.0;
        output.width = column_width * columns as f32;
        for (r, cells) in table.rows.iter().enumerate() {
            let top = output.height;
            let mut row_height = options.line_height + padding * 2.0;
            let mut layouts = Vec::new();
            for cell in cells {
                let local: Vec<_> = spans
                    .iter()
                    .filter_map(|span| {
                        let start = span.range.start.max(cell.start);
                        let end = span.range.end.min(cell.end);
                        (start < end).then(|| TextStyleRange {
                            range: ByteOffset(start.0 - cell.start.0)
                                ..ByteOffset(end.0 - cell.start.0),
                            style: if r == 0 {
                                FontStyle {
                                    weight: span.style.weight.max(700),
                                    ..span.style
                                }
                            } else {
                                span.style
                            },
                        })
                    })
                    .collect();
                let mut base = font;
                if r == 0 {
                    base.weight = 700;
                }
                let shaped = self.shape_styled(
                    &text[cell.start.0..cell.end.0],
                    LayoutOptions {
                        width: (column_width - padding * 2.0).max(1.0),
                        ..options
                    },
                    base,
                    &local,
                )?;
                row_height = row_height.max(shaped.height + padding * 2.0);
                layouts.push(shaped);
            }
            for (c, (cell, mut shaped)) in cells.iter().zip(layouts).enumerate() {
                let left = c as f32 * column_width;
                for row in &mut shaped.rows {
                    let used = row.carets.iter().map(|caret| caret.x).fold(0.0, f32::max);
                    let free = (column_width - padding * 2.0 - used).max(0.0);
                    let align = match table.alignments[c] {
                        ColumnAlignment::Left => 0.0,
                        ColumnAlignment::Center => free * 0.5,
                        ColumnAlignment::Right => free,
                    };
                    let dx = left + padding + align;
                    let old_top = row.bounds.y;
                    for glyph in &mut shaped.glyphs {
                        if row.source_range.start <= glyph.source_range.start
                            && glyph.source_range.end <= row.source_range.end
                        {
                            glyph.bounds.x += dx;
                            glyph.bounds.y += top + padding;
                        }
                    }
                    row.source_range = ByteOffset(row.source_range.start.0 + cell.start.0)
                        ..ByteOffset(row.source_range.end.0 + cell.start.0);
                    row.bounds.x = left;
                    row.bounds.y = old_top + top + padding;
                    row.bounds.width = column_width;
                    for caret in &mut row.carets {
                        caret.byte.0 += cell.start.0;
                        caret.x += dx;
                        caret.y += top + padding;
                    }
                    for cluster in &mut row.clusters {
                        cluster.source_range.start.0 += cell.start.0;
                        cluster.source_range.end.0 += cell.start.0;
                        cluster.bounds.x += dx;
                        cluster.bounds.y += top + padding;
                    }
                }
                for glyph in &mut shaped.glyphs {
                    glyph.source_range.start.0 += cell.start.0;
                    glyph.source_range.end.0 += cell.start.0;
                }
                output.diagnostics.font_faces = output
                    .diagnostics
                    .font_faces
                    .max(shaped.diagnostics.font_faces);
                output.diagnostics.glyph_count += shaped.diagnostics.glyph_count;
                output.diagnostics.missing_glyphs += shaped.diagnostics.missing_glyphs;
                output.diagnostics.raster_failures += shaped.diagnostics.raster_failures;
                output.diagnostics.color_glyphs += shaped.diagnostics.color_glyphs;
                output.diagnostics.style_runs_visited += shaped.diagnostics.style_runs_visited;
                output.rows.extend(shaped.rows);
                output.glyphs.extend(shaped.glyphs);
                output.table_cells.push(TableCellGeometry {
                    range: cell.clone(),
                    bounds: Rect {
                        x: left,
                        y: top,
                        width: column_width,
                        height: row_height,
                    },
                    row: r,
                    column: c,
                });
            }
            output.height += row_height;
        }
        if text.ends_with('\n') {
            let mut tail = self.shape_styled("", options, font, &[])?;
            let y = output.height + 8.0;
            for row in &mut tail.rows {
                row.source_range = ByteOffset(text.len())..ByteOffset(text.len());
                row.bounds.y += y;
                for caret in &mut row.carets {
                    caret.byte = ByteOffset(text.len());
                    caret.y += y;
                }
            }
            output.height = y + tail.height;
            output.rows.extend(tail.rows);
        }
        output.rows.sort_by(|a, b| {
            a.bounds
                .y
                .total_cmp(&b.bounds.y)
                .then(a.bounds.x.total_cmp(&b.bounds.x))
        });
        Ok(output)
    }
}
