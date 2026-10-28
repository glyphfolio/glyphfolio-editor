use std::{collections::HashMap, ops::Range, path::Path, sync::Arc};

use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, FontSystem, Metrics, Shaping, Style, SwashCache, SwashContent,
    Weight, Wrap,
};
use glyphfolio_editor_core::{Affinity, ByteOffset};
use unicode_segmentation::UnicodeSegmentation;

static NEXT_SHAPER_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShapingGeneration(u64, u64);

/// All geometry is in logical display units, independent of a UI toolkit.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn right(self) -> f32 {
        self.x + self.width
    }
    pub fn bottom(self) -> f32 {
        self.y + self.height
    }
    pub fn center(self) -> Point {
        Point {
            x: self.x + self.width * 0.5,
            y: self.y + self.height * 0.5,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LayoutOptions {
    pub width: f32,
    pub font_size: f32,
    pub line_height: f32,
    pub scale: f32,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            width: 800.0,
            font_size: 19.0,
            line_height: 28.0,
            scale: 1.0,
        }
    }
}

/// Geometry-affecting font choices. Discovery and fallback remain host-local.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FontFamily {
    #[default]
    SansSerif,
    Serif,
    Monospace,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FontStyle {
    pub family: FontFamily,
    pub weight: u16,
    pub italic: bool,
}

impl Default for FontStyle {
    fn default() -> Self {
        Self {
            family: FontFamily::SansSerif,
            weight: 400,
            italic: false,
        }
    }
}

/// Non-overlapping font runs in the supplied display text's UTF-8 coordinates.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TextStyleRange {
    pub range: Range<ByteOffset>,
    pub style: FontStyle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidStyleRange;

impl std::fmt::Display for InvalidStyleRange {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .write_str("font runs must be sorted, non-overlapping UTF-8 ranges within display text")
    }
}
impl std::error::Error for InvalidStyleRange {}

fn font_attrs<'a>(style: FontStyle, sans: &'a str, serif: &'a str, mono: &'a str) -> Attrs<'a> {
    Attrs::new()
        .family(match style.family {
            FontFamily::SansSerif => Family::Name(sans),
            FontFamily::Serif => Family::Name(serif),
            FontFamily::Monospace => Family::Name(mono),
        })
        .weight(Weight(style.weight.clamp(1, 1000)))
        .style(if style.italic {
            Style::Italic
        } else {
            Style::Normal
        })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaretStop {
    pub byte: ByteOffset,
    pub affinity: Affinity,
    pub x: f32,
    pub y: f32,
    pub height: f32,
}

impl CaretStop {
    pub fn rect(self) -> Rect {
        Rect {
            x: self.x,
            y: self.y,
            width: 1.5,
            height: self.height,
        }
    }
}

/// Source clusters and style-independent geometric regions. One ligature can
/// span several legal grapheme stops; source spelling remains authoritative.
#[derive(Clone, Debug)]
pub struct GlyphCluster {
    pub source_range: Range<ByteOffset>,
    pub bounds: Rect,
    pub rtl: bool,
}

#[derive(Clone, Debug)]
pub struct ShapedRow {
    pub source_range: Range<ByteOffset>,
    pub logical_line: usize,
    pub visual_index: usize,
    pub bounds: Rect,
    pub rtl: bool,
    /// Sorted by visual X. Bidi boundaries may have two positions for one byte.
    pub carets: Vec<CaretStop>,
    pub clusters: Vec<GlyphCluster>,
}

/// Mask images contain white RGB plus alpha; color glyphs retain their own RGBA
/// and must not be tinted by the embedding host's text color.
#[derive(Clone, Debug)]
pub struct GlyphImage {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub color: bool,
}

#[derive(Clone, Debug)]
pub struct PositionedGlyph {
    pub source_range: Range<ByteOffset>,
    pub bounds: Rect,
    pub image: Arc<GlyphImage>,
}

#[derive(Clone, Debug, Default)]
pub struct LayoutDiagnostics {
    pub missing_glyphs: usize,
    pub raster_failures: usize,
    pub color_glyphs: usize,
    pub glyph_count: usize,
    pub font_faces: usize,
    /// Disjoint font runs intersecting source lines, excluding binary searches.
    pub style_runs_visited: usize,
}

#[derive(Clone, Debug)]
pub struct ShapedDocument {
    pub text: String,
    pub rows: Vec<ShapedRow>,
    pub glyphs: Vec<PositionedGlyph>,
    pub width: f32,
    pub height: f32,
    pub diagnostics: LayoutDiagnostics,
    pub table_cells: Vec<crate::TableCellGeometry>,
}

impl ShapedDocument {
    /// Original hit testing resolves legal source grapheme boundaries in visual order.
    pub fn hit_test(&self, point: Point) -> CaretStop {
        let cell = self.table_cells.iter().find(|cell| {
            cell.bounds.x <= point.x
                && point.x <= cell.bounds.right()
                && cell.bounds.y <= point.y
                && point.y <= cell.bounds.bottom()
        });
        let row = self
            .rows
            .iter()
            .filter(|row| {
                cell.is_none_or(|cell| {
                    cell.range.start <= row.source_range.start
                        && row.source_range.end <= cell.range.end
                })
            })
            .min_by(|a, b| {
                distance_y(a.bounds, point.y)
                    .total_cmp(&distance_y(b.bounds, point.y))
                    .then(distance_x(a.bounds, point.x).total_cmp(&distance_x(b.bounds, point.x)))
            })
            .expect("layout always includes an empty row");
        *row.carets
            .iter()
            .min_by(|a, b| (a.x - point.x).abs().total_cmp(&(b.x - point.x).abs()))
            .expect("every row has a caret")
    }

    pub fn caret(&self, byte: ByteOffset, affinity: Affinity) -> CaretStop {
        // Wrap and bidi boundaries can have distinct Before/After positions.
        let mut candidates: Vec<_> = self
            .rows
            .iter()
            .flat_map(|row| row.carets.iter())
            .filter(|caret| caret.byte == byte)
            .copied()
            .collect();
        if affinity == Affinity::After {
            candidates.reverse();
        }
        candidates
            .iter()
            .find(|caret| caret.affinity == affinity)
            .copied()
            .or_else(|| candidates.first().copied())
            .unwrap_or_else(|| {
                self.rows
                    .iter()
                    .flat_map(|row| &row.carets)
                    .min_by_key(|caret| caret.byte.0.abs_diff(byte.0))
                    .copied()
                    .expect("layout has a caret")
            })
    }

    pub fn caret_rect(&self, byte: ByteOffset, affinity: Affinity) -> Rect {
        self.caret(byte, affinity).rect()
    }

    pub fn selection_rects(&self, selected: Range<ByteOffset>) -> Vec<Rect> {
        if selected.start >= selected.end {
            return Vec::new();
        }
        let legal = source_grapheme_boundaries(&self.text);
        let mut output = Vec::new();
        for row in &self.rows {
            let mut pieces = Vec::new();
            for cluster in &row.clusters {
                if selected.start >= cluster.source_range.end
                    || selected.end <= cluster.source_range.start
                {
                    continue;
                }
                let boundaries = cluster_boundaries(&legal, cluster.source_range.clone());
                for (index, pair) in boundaries.windows(2).enumerate() {
                    if selected.start.0 >= pair[1] || selected.end.0 <= pair[0] {
                        continue;
                    }
                    let count = boundaries.len().saturating_sub(1).max(1) as f32;
                    let left_index = if cluster.rtl {
                        count - index as f32 - 1.0
                    } else {
                        index as f32
                    };
                    pieces.push(Rect {
                        x: cluster.bounds.x + cluster.bounds.width * left_index / count,
                        y: row.bounds.y,
                        width: cluster.bounds.width / count,
                        height: row.bounds.height,
                    });
                }
            }
            if row.source_range.end > row.source_range.start
                && self.text[row.source_range.start.0..row.source_range.end.0]
                    .ends_with(['\r', '\n'])
            {
                let end = row
                    .clusters
                    .iter()
                    .map(|cluster| cluster.source_range.end.0)
                    .max()
                    .unwrap_or(row.source_range.start.0);
                if selected.start.0 < row.source_range.end.0 && selected.end.0 > end {
                    let caret = row
                        .carets
                        .iter()
                        .find(|caret| caret.byte.0 == end)
                        .copied()
                        .unwrap_or(row.carets[0]);
                    pieces.push(Rect {
                        x: caret.x,
                        y: row.bounds.y,
                        width: 3.0,
                        height: row.bounds.height,
                    });
                }
            }
            pieces.sort_by(|a, b| a.x.total_cmp(&b.x));
            let mut merged: Vec<Rect> = Vec::new();
            for piece in pieces {
                if let Some(previous) = merged
                    .last_mut()
                    .filter(|previous| piece.x <= previous.right() + 0.5)
                {
                    previous.width = previous.right().max(piece.right()) - previous.x;
                } else {
                    merged.push(piece);
                }
            }
            output.extend(merged);
        }
        output
    }

    pub fn visual_neighbor(
        &self,
        byte: ByteOffset,
        affinity: Affinity,
        forward: bool,
    ) -> CaretStop {
        let current = self.caret(byte, affinity);
        let row_index =
            self.rows
                .iter()
                .position(|row| {
                    (row.bounds.y - current.y).abs() < 0.1
                        && row.carets.iter().any(|stop| {
                            stop.byte == current.byte && (stop.x - current.x).abs() < 0.1
                        })
                })
                .unwrap_or(0);
        let stops = visual_stops(&self.rows[row_index]);
        let index = stops
            .iter()
            .position(|stop| (stop.x - current.x).abs() < 0.1 && stop.byte == current.byte)
            .unwrap_or_else(|| {
                stops
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| {
                        (a.x - current.x).abs().total_cmp(&(b.x - current.x).abs())
                    })
                    .map_or(0, |(index, _)| index)
            });
        if forward {
            stops
                .get(index + 1)
                .copied()
                .or_else(|| self.rows.get(row_index + 1).map(|row| visual_stops(row)[0]))
                .unwrap_or(current)
        } else {
            index
                .checked_sub(1)
                .and_then(|index| stops.get(index).copied())
                .or_else(|| {
                    row_index
                        .checked_sub(1)
                        .map(|index| *visual_stops(&self.rows[index]).last().expect("caret"))
                })
                .unwrap_or(current)
        }
    }

    pub fn vertical_target(
        &self,
        byte: ByteOffset,
        affinity: Affinity,
        down: bool,
        preferred_x: f32,
    ) -> CaretStop {
        let current = self.caret(byte, affinity);
        let column = self
            .table_cells
            .iter()
            .find(|cell| cell.range.start <= byte && byte <= cell.range.end)
            .map(|cell| cell.column);
        let same_column = |row: &&ShapedRow| {
            column.is_none_or(|column| {
                self.table_cells.iter().any(|cell| {
                    cell.column == column
                        && cell.range.start <= row.source_range.start
                        && row.source_range.end <= cell.range.end
                })
            })
        };
        let target_y = self
            .rows
            .iter()
            .filter(same_column)
            .filter(|row| {
                if down {
                    row.bounds.y > current.y + 0.1
                } else {
                    row.bounds.y < current.y - 0.1
                }
            })
            .map(|row| row.bounds.y)
            .min_by(|a, b| (a - current.y).abs().total_cmp(&(b - current.y).abs()));
        let Some(y) = target_y else {
            return current;
        };
        self.rows
            .iter()
            .filter(same_column)
            .filter(|row| (row.bounds.y - y).abs() < 0.1)
            .flat_map(|row| &row.carets)
            .min_by(|a, b| {
                (a.x - preferred_x)
                    .abs()
                    .total_cmp(&(b.x - preferred_x).abs())
            })
            .copied()
            .unwrap_or(current)
    }

    pub fn row_edge(&self, byte: ByteOffset, affinity: Affinity, end: bool) -> CaretStop {
        let current = self.caret(byte, affinity);
        let row =
            self.rows
                .iter()
                .find(|row| {
                    (row.bounds.y - current.y).abs() < 0.1
                        && row.carets.iter().any(|stop| {
                            stop.byte == current.byte && (stop.x - current.x).abs() < 0.1
                        })
                })
                .unwrap_or(&self.rows[0]);
        if end {
            *row.carets.last().expect("caret")
        } else {
            row.carets[0]
        }
    }
}

fn visual_stops(row: &ShapedRow) -> Vec<CaretStop> {
    let mut stops = row.carets.clone();
    stops.dedup_by(|a, b| (a.x - b.x).abs() < 0.1);
    stops
}
fn distance_x(rect: Rect, x: f32) -> f32 {
    if x < rect.x {
        rect.x - x
    } else if x > rect.right() {
        x - rect.right()
    } else {
        0.0
    }
}

fn distance_y(rect: Rect, y: f32) -> f32 {
    if y < rect.y {
        rect.y - y
    } else if y > rect.bottom() {
        y - rect.bottom()
    } else {
        0.0
    }
}
fn source_grapheme_boundaries(text: &str) -> Vec<usize> {
    text.grapheme_indices(true)
        .map(|(offset, _)| offset)
        .chain([text.len()])
        .collect()
}

/// Include actual shaping-cluster edges for interpolation, even when an edge
/// falls inside an extended source grapheme. Those synthetic edges describe
/// geometry only; the public caret loop separately rejects illegal EGC stops.
/// Without them an end-only legal boundary is incorrectly placed at X-start.
fn cluster_boundaries(legal: &[usize], range: Range<ByteOffset>) -> Vec<usize> {
    let mut boundaries = vec![range.start.0];
    let from = legal.partition_point(|offset| *offset <= range.start.0);
    let to = legal.partition_point(|offset| *offset < range.end.0);
    if from < to {
        boundaries.extend_from_slice(&legal[from..to]);
    }
    if range.end > range.start {
        boundaries.push(range.end.0);
    }
    boundaries
}

/// Private font/raster state. Native cache keys never enter the public contract.
pub struct ShapingEngine {
    fonts: FontSystem,
    cache: SwashCache,
    images: HashMap<CacheKey, Arc<GlyphImage>>,
    next_image: u64,
    family: String,
    serif_family: String,
    mono_family: String,
    image_bytes: usize,
    raster_queries: usize,
    reset_raster_pending: bool,
    generation: ShapingGeneration,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RasterCacheStats {
    pub image_entries: usize,
    pub rgba_bytes: usize,
    pub queries_since_reset: usize,
}
impl Default for ShapingEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ShapingEngine {
    pub fn new() -> Self {
        let mut fonts = FontSystem::new();
        let family = ["Arial", "Noto Sans", "DejaVu Sans", "Liberation Sans"]
            .into_iter()
            .find(|family| {
                fonts
                    .db()
                    .faces()
                    .any(|face| face.families.iter().any(|(name, _)| name == family))
            })
            .unwrap_or("sans-serif")
            .to_owned();
        fonts.db_mut().set_sans_serif_family(family.clone());
        let choose = |names: &[&str]| {
            names
                .iter()
                .find(|name| {
                    fonts
                        .db()
                        .faces()
                        .any(|face| face.families.iter().any(|(family, _)| family == **name))
                })
                .map_or_else(|| family.clone(), |name| (*name).to_owned())
        };
        let serif_family = choose(&[
            "Times New Roman",
            "Noto Serif",
            "DejaVu Serif",
            "Liberation Serif",
            "Times",
        ]);
        let mono_family = choose(&[
            "Menlo",
            "SFMono-Regular",
            "Consolas",
            "DejaVu Sans Mono",
            "Liberation Mono",
            "Courier New",
        ]);
        fonts.db_mut().set_serif_family(serif_family.clone());
        fonts.db_mut().set_monospace_family(mono_family.clone());
        Self {
            fonts,
            cache: SwashCache::new(),
            images: HashMap::new(),
            next_image: 1,
            family,
            serif_family,
            mono_family,
            image_bytes: 0,
            raster_queries: 0,
            reset_raster_pending: false,
            generation: ShapingGeneration(
                NEXT_SHAPER_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                0,
            ),
        }
    }
    pub fn font_faces(&self) -> usize {
        self.fonts.db().faces().count()
    }
    pub fn locale(&self) -> &str {
        self.fonts.locale()
    }
    pub fn primary_family(&self) -> &str {
        &self.family
    }
    pub fn cache_generation(&self) -> ShapingGeneration {
        self.generation
    }
    pub fn raster_cache_stats(&self) -> RasterCacheStats {
        RasterCacheStats {
            image_entries: self.images.len(),
            rgba_bytes: self.image_bytes,
            queries_since_reset: self.raster_queries,
        }
    }
    pub fn clear_raster_cache(&mut self) {
        self.images.clear();
        self.cache = SwashCache::new();
        self.image_bytes = 0;
        self.raster_queries = 0;
        self.reset_raster_pending = false;
    }
    fn bound_raster_cache(&mut self) {
        if self.reset_raster_pending
            || self.images.len() >= 4096
            || self.image_bytes >= 32 * 1024 * 1024
            || self.raster_queries >= 8192
        {
            self.clear_raster_cache();
        }
    }
    /// A host-local font is loaded, not redistributed with this crate.
    pub fn load_font_file(&mut self, path: &Path) -> Result<(), String> {
        self.fonts
            .db_mut()
            .load_font_file(path)
            .map_err(|error| error.to_string())?;
        self.clear_raster_cache();
        self.generation.1 = self.generation.1.wrapping_add(1);
        Ok(())
    }

    /// Full small-document layout. Geometry is logical; raster images use physical
    /// `scale` pixels per logical unit. Source bytes are never rewritten.
    pub fn shape(&mut self, text: &str, options: LayoutOptions) -> ShapedDocument {
        self.shape_impl(text, options, FontStyle::default(), &[])
    }

    pub fn shape_with_style(
        &mut self,
        text: &str,
        options: LayoutOptions,
        style: FontStyle,
    ) -> ShapedDocument {
        self.shape_impl(text, options, style, &[])
    }

    /// Shape nested Markdown styles after the projection has flattened them into
    /// disjoint display runs. Invalid ranges are refused before any text slicing.
    pub fn shape_styled(
        &mut self,
        text: &str,
        options: LayoutOptions,
        base: FontStyle,
        styles: &[TextStyleRange],
    ) -> Result<ShapedDocument, InvalidStyleRange> {
        let mut end = 0;
        for span in styles {
            if span.range.start.0 < end
                || span.range.start >= span.range.end
                || span.range.end.0 > text.len()
                || !text.is_char_boundary(span.range.start.0)
                || !text.is_char_boundary(span.range.end.0)
            {
                return Err(InvalidStyleRange);
            }
            end = span.range.end.0;
        }
        Ok(self.shape_impl(text, options, base, styles))
    }

    fn shape_impl(
        &mut self,
        text: &str,
        options: LayoutOptions,
        base: FontStyle,
        styles: &[TextStyleRange],
    ) -> ShapedDocument {
        // Small-document feasibility caches have a bounded entry population;
        // callers still own the lifetime of any returned image Arcs.
        self.bound_raster_cache();
        let width = finite_positive(options.width, 800.0).max(1.0);
        let font_size = finite_positive(options.font_size, 19.0).clamp(1.0, 512.0);
        let line_height = finite_positive(options.line_height, 28.0).max(font_size);
        let scale = finite_positive(options.scale, 1.0).clamp(0.25, 8.0);
        let legal = source_grapheme_boundaries(text);
        let mut document = ShapedDocument {
            text: text.to_owned(),
            table_cells: Vec::new(),
            rows: Vec::new(),
            glyphs: Vec::new(),
            width,
            height: 0.0,
            diagnostics: LayoutDiagnostics {
                font_faces: self.font_faces(),
                ..Default::default()
            },
        };
        let mut y = 0.0;
        for (logical_line, (source, ending)) in source_lines(text).into_iter().enumerate() {
            let line = &text[source.clone()];
            let mut buffer = Buffer::new(&mut self.fonts, Metrics::new(font_size, line_height));
            buffer.set_size(Some(width), None);
            buffer.set_wrap(Wrap::WordOrGlyph);
            buffer.set_tab_width(4);
            let attrs = font_attrs(base, &self.family, &self.serif_family, &self.mono_family);
            if styles.is_empty() {
                buffer.set_text(line, &attrs, Shaping::Advanced, None);
            } else {
                let mut spans = Vec::new();
                let mut at = source.start;
                let first = styles.partition_point(|span| span.range.end.0 <= source.start);
                for span in styles[first..]
                    .iter()
                    .take_while(|span| span.range.start.0 < source.end)
                {
                    document.diagnostics.style_runs_visited += 1;
                    let start = span.range.start.0.max(source.start);
                    let end = span.range.end.0.min(source.end);
                    if start >= end {
                        continue;
                    }
                    if at < start {
                        spans.push((&text[at..start], attrs.clone()));
                    }
                    spans.push((
                        &text[start..end],
                        font_attrs(
                            span.style,
                            &self.family,
                            &self.serif_family,
                            &self.mono_family,
                        ),
                    ));
                    at = end;
                }
                if at < source.end {
                    spans.push((&text[at..source.end], attrs.clone()));
                }
                buffer.set_rich_text(spans, &attrs, Shaping::Advanced, None);
            }
            buffer.shape_until_scroll(&mut self.fonts, false);
            let runs: Vec<_> = buffer.layout_runs().collect();
            let run_count = runs.len();
            let mut line_bottom: f32 = line_height;
            for (visual_index, run) in runs.into_iter().enumerate() {
                let mut clusters: Vec<GlyphCluster> = Vec::new();
                for glyph in run.glyphs {
                    self.bound_raster_cache();
                    let range = ByteOffset(source.start + glyph.start)
                        ..ByteOffset(source.start + glyph.end);
                    let bounds = Rect {
                        x: glyph.x,
                        y: y + run.line_top,
                        width: glyph.w.max(0.0),
                        height: run.line_height,
                    };
                    if let Some(cluster) = clusters.iter_mut().find(|cluster| {
                        cluster.source_range == range && cluster.rtl == glyph.level.is_rtl()
                    }) {
                        let left = cluster.bounds.x.min(bounds.x);
                        let right = cluster.bounds.right().max(bounds.right());
                        cluster.bounds.x = left;
                        cluster.bounds.width = right - left;
                    } else {
                        self.raster_queries += 1;
                        clusters.push(GlyphCluster {
                            source_range: range.clone(),
                            bounds,
                            rtl: glyph.level.is_rtl(),
                        });
                    }
                    document.diagnostics.glyph_count += 1;
                    let visible = line[glyph.start..glyph.end].chars().any(|c| {
                        !c.is_whitespace()
                            && !c.is_control()
                            && !matches!(c, '\u{200d}' | '\u{fe0f}' | '\u{feff}')
                    });
                    if glyph.glyph_id == 0 && visible {
                        document.diagnostics.missing_glyphs += 1;
                    }
                    let physical = glyph.physical((0.0, (y + run.line_y) * scale), scale);
                    let image = if let Some(image) = self.images.get(&physical.cache_key) {
                        Some(image.clone())
                    } else {
                        self.cache
                            .get_image(&mut self.fonts, physical.cache_key)
                            .as_ref()
                            .and_then(|raster| {
                                if raster.placement.width == 0 || raster.placement.height == 0 {
                                    return None;
                                }
                                let color = raster.content == SwashContent::Color;
                                let rgba = match raster.content {
                                    SwashContent::Mask => raster
                                        .data
                                        .iter()
                                        .flat_map(|alpha| [255, 255, 255, *alpha])
                                        .collect(),
                                    SwashContent::Color => raster.data.clone(),
                                    SwashContent::SubpixelMask => raster
                                        .data
                                        .as_chunks::<4>()
                                        .0
                                        .iter()
                                        .flat_map(|pixel| {
                                            [255, 255, 255, pixel[0].max(pixel[1]).max(pixel[2])]
                                        })
                                        .collect(),
                                };
                                let image = Arc::new(GlyphImage {
                                    id: self.next_image,
                                    width: raster.placement.width,
                                    height: raster.placement.height,
                                    rgba,
                                    color,
                                });
                                self.next_image += 1;
                                if self.image_bytes.saturating_add(image.rgba.capacity())
                                    <= 32 * 1024 * 1024
                                {
                                    self.image_bytes += image.rgba.capacity();
                                    self.images.insert(physical.cache_key, image.clone());
                                } else {
                                    self.reset_raster_pending = true;
                                }
                                Some(image)
                            })
                    };
                    if let Some(image) = image {
                        let raster = self
                            .cache
                            .get_image(&mut self.fonts, physical.cache_key)
                            .as_ref()
                            .expect("cached raster");
                        let bounds = Rect {
                            x: (physical.x + raster.placement.left) as f32 / scale,
                            y: (physical.y - raster.placement.top) as f32 / scale,
                            width: image.width as f32 / scale,
                            height: image.height as f32 / scale,
                        };
                        if image.color {
                            document.diagnostics.color_glyphs += 1;
                        }
                        document.glyphs.push(PositionedGlyph {
                            source_range: range,
                            bounds,
                            image,
                        });
                    } else if visible && glyph.glyph_id != 0 && glyph.w > 0.0 {
                        document.diagnostics.raster_failures += 1;
                    }
                }
                let start = clusters
                    .iter()
                    .map(|cluster| cluster.source_range.start.0)
                    .min()
                    .unwrap_or(source.start);
                let end = clusters
                    .iter()
                    .map(|cluster| cluster.source_range.end.0)
                    .max()
                    .unwrap_or(source.start);
                let source_end = if visual_index + 1 == run_count {
                    ending.end
                } else {
                    end
                };
                let bounds = Rect {
                    x: 0.0,
                    y: y + run.line_top,
                    width: run.line_w.max(1.0),
                    height: run.line_height,
                };
                let mut carets = Vec::new();
                for cluster in &clusters {
                    let boundaries = cluster_boundaries(&legal, cluster.source_range.clone());
                    let count = boundaries.len().saturating_sub(1).max(1) as f32;
                    for (index, boundary) in boundaries.iter().enumerate() {
                        if legal.binary_search(boundary).is_err() {
                            continue;
                        }
                        let ratio = index as f32 / count;
                        let x = if cluster.rtl {
                            cluster.bounds.right() - cluster.bounds.width * ratio
                        } else {
                            cluster.bounds.x + cluster.bounds.width * ratio
                        };
                        carets.push(CaretStop {
                            byte: ByteOffset(*boundary),
                            affinity: if *boundary == cluster.source_range.start.0 {
                                Affinity::After
                            } else {
                                Affinity::Before
                            },
                            x,
                            y: bounds.y,
                            height: bounds.height,
                        });
                    }
                }
                if carets.is_empty() {
                    carets.push(CaretStop {
                        byte: ByteOffset(start),
                        affinity: Affinity::After,
                        x: if run.rtl { width } else { 0.0 },
                        y: bounds.y,
                        height: bounds.height,
                    });
                }
                carets.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.byte.cmp(&b.byte)));
                carets.dedup_by(|a, b| {
                    a.byte == b.byte && a.affinity == b.affinity && (a.x - b.x).abs() < 0.01
                });
                document.rows.push(ShapedRow {
                    source_range: ByteOffset(start)..ByteOffset(source_end),
                    logical_line,
                    visual_index,
                    bounds,
                    rtl: run.rtl,
                    carets,
                    clusters,
                });
                line_bottom = line_bottom.max(run.line_top + run.line_height);
            }
            y += line_bottom;
        }
        document.height = y;
        self.bound_raster_cache();
        document
    }
}

/// Original source split keeps CRLF atomic and a final empty caret row.
fn source_lines(text: &str) -> Vec<(Range<usize>, Range<usize>)> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut at = 0;
    while at < bytes.len() {
        if matches!(bytes[at], b'\r' | b'\n') {
            let end = at;
            at += 1;
            if bytes[end] == b'\r' && bytes.get(at) == Some(&b'\n') {
                at += 1;
            }
            lines.push((start..end, end..at));
            start = at;
        } else {
            at += 1;
        }
    }
    lines.push((start..text.len(), text.len()..text.len()));
    lines
}

fn finite_positive(value: f32, fallback: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        fallback
    }
}
