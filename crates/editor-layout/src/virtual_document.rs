//! Original block flow and bounded visible shaping. No GUI or editor controller.

use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    ops::Range,
    sync::Arc,
};

use glyphfolio_editor_core::{Affinity, ByteOffset};

use crate::{
    CaretStop, FontStyle, LayoutOptions, Point, Rect, ShapedDocument, ShapingEngine, TextStyleRange,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LayoutBlockId(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Insets {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockStyle {
    pub font: FontStyle,
    pub font_size: f32,
    pub line_height: f32,
    pub indent: f32,
    pub margin_before: f32,
    pub margin_after: f32,
    pub padding: Insets,
}

impl Default for BlockStyle {
    fn default() -> Self {
        Self {
            font: FontStyle::default(),
            font_size: 19.0,
            line_height: 28.0,
            indent: 0.0,
            margin_before: 0.0,
            margin_after: 0.0,
            padding: Insets::default(),
        }
    }
}

/// One semantic leaf or exact hard-line fragment. A shared full projection avoids
/// allocating a new String for every block whenever a document revision changes.
#[derive(Clone, Debug)]
pub struct LayoutBlock {
    pub id: LayoutBlockId,
    pub generation: u64,
    pub source_range: Range<ByteOffset>,
    pub display_range: Range<ByteOffset>,
    pub text: Arc<str>,
    pub text_range: Range<usize>,
    pub style: BlockStyle,
    /// Local display byte offsets, flattened into disjoint ranges by projection.
    pub spans: Arc<[TextStyleRange]>,
    pub table: Option<Arc<crate::TableLayoutSpec>>,
}

impl LayoutBlock {
    pub fn from_shared(
        id: LayoutBlockId,
        generation: u64,
        source_range: Range<ByteOffset>,
        display_range: Range<ByteOffset>,
        text: Arc<str>,
        style: BlockStyle,
    ) -> Self {
        Self {
            id,
            generation,
            source_range,
            text_range: display_range.start.0..display_range.end.0,
            display_range,
            text,
            style,
            spans: Arc::from([]),
            table: None,
        }
    }
    pub fn content(&self) -> &str {
        &self.text[self.text_range.clone()]
    }
    pub fn with_spans(mut self, spans: impl Into<Arc<[TextStyleRange]>>) -> Self {
        self.spans = spans.into();
        self
    }

    pub fn with_table(mut self, table: crate::TableLayoutSpec) -> Self {
        self.table = Some(Arc::new(table));
        self
    }

    fn validate(&self) -> Result<(), LayoutError> {
        if self
            .table
            .as_ref()
            .is_some_and(|table| !table.valid(self.content()))
        {
            return Err(LayoutError::InvalidBlock(self.id));
        }
        if self.source_range.start > self.source_range.end
            || self.display_range.start > self.display_range.end
            || self.text_range.start > self.text_range.end
            || self.text_range.end > self.text.len()
            || !self.text.is_char_boundary(self.text_range.start)
            || !self.text.is_char_boundary(self.text_range.end)
            || self.display_range.end.0 - self.display_range.start.0 != self.text_range.len()
        {
            return Err(LayoutError::InvalidBlock(self.id));
        }
        let mut end = 0;
        for span in self.spans.iter() {
            if span.range.start.0 < end
                || span.range.start >= span.range.end
                || span.range.end.0 > self.text_range.len()
                || !self.content().is_char_boundary(span.range.start.0)
                || !self.content().is_char_boundary(span.range.end.0)
            {
                return Err(LayoutError::InvalidBlock(self.id));
            }
            end = span.range.end.0;
        }
        let s = self.style;
        if !s.font_size.is_finite()
            || !(1.0..=512.0).contains(&s.font_size)
            || !s.line_height.is_finite()
            || s.line_height < s.font_size
            || s.line_height > 4096.0
            || [
                s.indent,
                s.margin_before,
                s.margin_after,
                s.padding.left,
                s.padding.right,
                s.padding.top,
                s.padding.bottom,
            ]
            .into_iter()
            .any(|v| !v.is_finite() || !(0.0..=1_000_000.0).contains(&v))
        {
            return Err(LayoutError::InvalidBlock(self.id));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutError {
    InvalidHeight,
    InvalidConfiguration,
    DuplicateBlock(LayoutBlockId),
    InvalidBlock(LayoutBlockId),
    UnorderedBlocks,
    MissingBlock,
    DeferredLongBlock,
}
impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "layout error: {self:?}")
    }
}
impl std::error::Error for LayoutError {}

/// Fenwick prefix sums: height update, Y lookup, and prefix lookup are O(log B).
/// Values are accumulated in f64 to avoid losing short rows far down a document.
#[derive(Clone, Debug, Default)]
pub struct HeightIndex {
    values: Vec<f32>,
    sums: Vec<f64>,
}

impl HeightIndex {
    pub fn new(values: impl IntoIterator<Item = f32>) -> Result<Self, LayoutError> {
        let values: Vec<_> = values.into_iter().collect();
        if values.iter().any(|v| !v.is_finite() || *v <= 0.0) {
            return Err(LayoutError::InvalidHeight);
        }
        let mut sums = vec![0.0; values.len() + 1];
        for (index, value) in values.iter().enumerate() {
            let i = index + 1;
            sums[i] += f64::from(*value);
            let parent = i + i.isolate_lowest_one();
            if parent < sums.len() {
                sums[parent] += sums[i];
            }
        }
        Ok(Self { values, sums })
    }
    pub fn len(&self) -> usize {
        self.values.len()
    }
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
    pub fn height(&self, index: usize) -> Option<f32> {
        self.values.get(index).copied()
    }
    pub fn prefix(&self, index: usize) -> f32 {
        let mut i = index.min(self.values.len());
        let mut total = 0.0;
        while i > 0 {
            total += self.sums[i];
            i &= i - 1;
        }
        total as f32
    }
    pub fn total(&self) -> f32 {
        self.prefix(self.values.len())
    }
    pub fn update(&mut self, index: usize, value: f32) -> Result<(), LayoutError> {
        if !value.is_finite() || value <= 0.0 {
            return Err(LayoutError::InvalidHeight);
        }
        let Some(old) = self.values.get_mut(index) else {
            return Err(LayoutError::MissingBlock);
        };
        let delta = f64::from(value) - f64::from(*old);
        *old = value;
        let mut i = index + 1;
        while i < self.sums.len() {
            self.sums[i] += delta;
            i += i.isolate_lowest_one();
        }
        Ok(())
    }
    /// The block containing Y, clamped to the first/last block outside bounds.
    pub fn block_at_y(&self, y: f32) -> Option<usize> {
        if self.values.is_empty() {
            return None;
        }
        let target = if y.is_finite() {
            y.max(0.0) as f64
        } else {
            0.0
        };
        let mut index = 0;
        let mut sum = 0.0;
        let mut step = 1usize << (usize::BITS - self.values.len().leading_zeros() - 1);
        while step > 0 {
            let next = index + step;
            if next < self.sums.len() && sum + self.sums[next] <= target {
                index = next;
                sum += self.sums[next];
            }
            step >>= 1;
        }
        Some(index.min(self.values.len() - 1))
    }
    pub fn intersecting(&self, top: f32, bottom: f32) -> Range<usize> {
        match (self.block_at_y(top), self.block_at_y(bottom.max(top))) {
            (Some(start), Some(end)) => start..(end + 1).min(self.len()),
            _ => 0..0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LongBlockPolicy {
    /// Correct full shaping, explicitly reported as exceeding the per-block byte
    /// threshold. This does not establish bounded latency for a giant paragraph.
    Exact,
    /// Keep an estimated placeholder. A host must resolve it off the frame path
    /// or offer source/hard-line fallback rather than silently truncate content.
    Defer,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutConfig {
    pub width: f32,
    pub scale: f32,
    pub font_generation: u64,
    pub typography_generation: u64,
    pub max_cached_blocks: usize,
    /// Conservative derived-data accounting, not total process/GPU memory.
    pub max_cached_bytes: usize,
    pub max_blocks_per_pass: usize,
    pub long_block_bytes: usize,
    pub long_block_policy: LongBlockPolicy,
}
impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            width: 800.0,
            scale: 1.0,
            font_generation: 0,
            typography_generation: 0,
            max_cached_blocks: 128,
            max_cached_bytes: 32 * 1024 * 1024,
            max_blocks_per_pass: 128,
            long_block_bytes: 32 * 1024,
            long_block_policy: LongBlockPolicy::Defer,
        }
    }
}
impl LayoutConfig {
    fn validate(self) -> Result<(), LayoutError> {
        if !self.width.is_finite()
            || self.width <= 0.0
            || !self.scale.is_finite()
            || self.scale <= 0.0
            || self.max_blocks_per_pass == 0
            || self.long_block_bytes == 0
        {
            return Err(LayoutError::InvalidConfiguration);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    pub top: f32,
    pub height: f32,
    pub overscan: f32,
}
impl Default for Viewport {
    fn default() -> Self {
        Self {
            top: 0.0,
            height: 600.0,
            overscan: 56.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BlockScrollAnchor {
    pub block_id: LayoutBlockId,
    /// Fallback source anchor; hosts map this through their committed change map.
    pub source_byte: ByteOffset,
    pub local_display_byte: ByteOffset,
    pub offset_y: f32,
}

#[derive(Clone, Debug, Default)]
pub struct VirtualLayoutStats {
    pub total_blocks: usize,
    pub blocks_considered: usize,
    pub shaped_blocks: usize,
    pub cache_hits: usize,
    pub cache_entries: usize,
    pub cache_bytes: usize,
    pub evicted_blocks: usize,
    pub largest_shaped_block_bytes: usize,
    pub exact_long_blocks: usize,
    pub deferred_long_blocks: usize,
    pub pending_blocks: usize,
}

#[derive(Clone, Debug)]
pub struct VisibleBlock {
    pub id: LayoutBlockId,
    pub index: usize,
    pub source_range: Range<ByteOffset>,
    pub display_range: Range<ByteOffset>,
    pub bounds: Rect,
    pub text_origin: Point,
    pub shaped: Arc<ShapedDocument>,
}
impl VisibleBlock {
    pub fn global_caret(&self, caret: CaretStop) -> CaretStop {
        CaretStop {
            byte: ByteOffset(self.display_range.start.0 + caret.byte.0),
            x: self.text_origin.x + caret.x,
            y: self.text_origin.y + caret.y,
            ..caret
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnmeasuredReason {
    PassBudget,
    LongBlock,
}
#[derive(Clone, Debug)]
pub struct UnmeasuredBlock {
    pub id: LayoutBlockId,
    pub index: usize,
    pub bounds: Rect,
    pub reason: UnmeasuredReason,
}

#[derive(Clone, Debug)]
pub struct ViewportLayout {
    pub blocks: Vec<VisibleBlock>,
    pub unmeasured: Vec<UnmeasuredBlock>,
    pub total_height: f32,
    pub height_is_exact: bool,
    pub corrected_scroll_top: f32,
    pub stats: VirtualLayoutStats,
}
impl ViewportLayout {
    /// Adapter compatibility bridge. Only measured rows/glyphs are included, but
    /// byte positions refer to the supplied full display string. This performs a
    /// full-text line scan and retains that String: callers needing strict frame
    /// bounds should use this type's block-local geometry methods directly.
    pub fn to_shaped_document(&self, text: String) -> ShapedDocument {
        let starts: Vec<_> = hard_line_ranges(&text)
            .into_iter()
            .map(|range| range.start)
            .collect();
        let mut rows = Vec::new();
        let mut glyphs = Vec::new();
        let mut table_cells = Vec::new();
        let mut diagnostics = crate::LayoutDiagnostics::default();
        for block in &self.blocks {
            let base = block.display_range.start.0;
            for row in &block.shaped.rows {
                let mut row = row.clone();
                row.source_range = ByteOffset(base + row.source_range.start.0)
                    ..ByteOffset(base + row.source_range.end.0);
                row.logical_line = starts
                    .partition_point(|start| *start <= row.source_range.start.0)
                    .saturating_sub(1);
                row.bounds.x += block.text_origin.x;
                row.bounds.y += block.text_origin.y;
                for caret in &mut row.carets {
                    caret.byte.0 += base;
                    caret.x += block.text_origin.x;
                    caret.y += block.text_origin.y;
                }
                for cluster in &mut row.clusters {
                    cluster.source_range.start.0 += base;
                    cluster.source_range.end.0 += base;
                    cluster.bounds.x += block.text_origin.x;
                    cluster.bounds.y += block.text_origin.y;
                }
                rows.push(row);
            }
            for glyph in &block.shaped.glyphs {
                let mut glyph = glyph.clone();
                glyph.source_range.start.0 += base;
                glyph.source_range.end.0 += base;
                glyph.bounds.x += block.text_origin.x;
                glyph.bounds.y += block.text_origin.y;
                glyphs.push(glyph);
            }
            for cell in &block.shaped.table_cells {
                let mut cell = cell.clone();
                cell.range.start.0 += base;
                cell.range.end.0 += base;
                cell.bounds.x += block.text_origin.x;
                cell.bounds.y += block.text_origin.y;
                table_cells.push(cell);
            }
            let d = &block.shaped.diagnostics;
            diagnostics.font_faces = diagnostics.font_faces.max(d.font_faces);
            diagnostics.glyph_count += d.glyph_count;
            diagnostics.missing_glyphs += d.missing_glyphs;
            diagnostics.raster_failures += d.raster_failures;
            diagnostics.color_glyphs += d.color_glyphs;
            diagnostics.style_runs_visited += d.style_runs_visited;
        }
        rows.sort_by(|a, b| {
            a.bounds
                .y
                .total_cmp(&b.bounds.y)
                .then(a.source_range.start.cmp(&b.source_range.start))
        });
        ShapedDocument {
            text,
            rows,
            glyphs,
            width: self
                .blocks
                .iter()
                .map(|block| block.text_origin.x + block.shaped.width)
                .fold(0.0, f32::max),
            table_cells,
            height: self.total_height,
            diagnostics,
        }
    }

    /// Coordinates and byte offsets here are document/display coordinates. A
    /// Markdown projection performs the final display-to-canonical-source map.
    pub fn hit_test(&self, point: Point) -> Option<CaretStop> {
        let block = self.blocks.iter().min_by(|a, b| {
            vertical_distance(a.bounds, point.y).total_cmp(&vertical_distance(b.bounds, point.y))
        })?;
        Some(block.global_caret(block.shaped.hit_test(Point {
            x: point.x - block.text_origin.x,
            y: point.y - block.text_origin.y,
        })))
    }
    pub fn caret(&self, byte: ByteOffset, affinity: Affinity) -> Option<CaretStop> {
        let mut candidates = self
            .blocks
            .iter()
            .filter(|block| block.display_range.start <= byte && byte <= block.display_range.end);
        let first = candidates.next()?;
        let block = if affinity == Affinity::After {
            candidates.next_back().unwrap_or(first)
        } else {
            first
        };
        Some(
            block.global_caret(
                block
                    .shaped
                    .caret(ByteOffset(byte.0 - block.display_range.start.0), affinity),
            ),
        )
    }
    pub fn selection_rects(&self, selected: Range<ByteOffset>) -> Vec<Rect> {
        let mut result = Vec::new();
        if selected.start >= selected.end {
            return result;
        }
        for block in &self.blocks {
            let start = selected.start.max(block.display_range.start);
            let end = selected.end.min(block.display_range.end);
            if start >= end {
                continue;
            }
            result.extend(
                block
                    .shaped
                    .selection_rects(
                        ByteOffset(start.0 - block.display_range.start.0)
                            ..ByteOffset(end.0 - block.display_range.start.0),
                    )
                    .into_iter()
                    .map(|r| Rect {
                        x: r.x + block.text_origin.x,
                        y: r.y + block.text_origin.y,
                        ..r
                    }),
            );
        }
        result
    }
    pub fn visual_neighbor(
        &self,
        byte: ByteOffset,
        affinity: Affinity,
        forward: bool,
    ) -> Option<CaretStop> {
        let current = self.caret(byte, affinity)?;
        let index = self.blocks.iter().position(|b| {
            b.display_range.start <= byte
                && byte <= b.display_range.end
                && current.y >= b.bounds.y
                && current.y < b.bounds.bottom()
        })?;
        let block = &self.blocks[index];
        let local = block.shaped.visual_neighbor(
            ByteOffset(byte.0 - block.display_range.start.0),
            affinity,
            forward,
        );
        let moved = block.global_caret(local);
        if moved.byte != current.byte
            || (moved.x - current.x).abs() > 0.1
            || (moved.y - current.y).abs() > 0.1
        {
            return Some(moved);
        }
        let next = if forward {
            self.blocks.get(index + 1)
        } else {
            index.checked_sub(1).and_then(|i| self.blocks.get(i))
        }?;
        if next.index.abs_diff(block.index) != 1 {
            return None;
        }
        let row = if forward {
            next.shaped.rows.first()?
        } else {
            next.shaped.rows.last()?
        };
        let stop = if forward {
            row.carets.first()?
        } else {
            row.carets.last()?
        };
        Some(next.global_caret(*stop))
    }
    pub fn vertical_target(
        &self,
        byte: ByteOffset,
        affinity: Affinity,
        down: bool,
        preferred_x: f32,
    ) -> Option<CaretStop> {
        let current = self.caret(byte, affinity)?;
        let rows: Vec<_> = self
            .blocks
            .iter()
            .flat_map(|block| block.shaped.rows.iter().map(move |row| (block, row)))
            .collect();
        let current_block = self
            .blocks
            .iter()
            .find(|block| block.display_range.start <= byte && byte <= block.display_range.end)?;
        let column = current_block
            .shaped
            .table_cells
            .iter()
            .find(|cell| {
                cell.range.start.0 + current_block.display_range.start.0 <= byte.0
                    && byte.0 <= cell.range.end.0 + current_block.display_range.start.0
            })
            .map(|cell| cell.column);
        let same_column = |block: &VisibleBlock, row: &crate::ShapedRow| {
            block.id != current_block.id
                || column.is_none_or(|column| {
                    block.shaped.table_cells.iter().any(|cell| {
                        cell.column == column
                            && cell.range.start <= row.source_range.start
                            && row.source_range.end <= cell.range.end
                    })
                })
        };
        let y = rows
            .iter()
            .filter(|(block, row)| same_column(block, row))
            .filter(|(block, row)| {
                if down {
                    block.text_origin.y + row.bounds.y > current.y + 0.1
                } else {
                    block.text_origin.y + row.bounds.y < current.y - 0.1
                }
            })
            .map(|(block, row)| block.text_origin.y + row.bounds.y)
            .min_by(|a, b| (a - current.y).abs().total_cmp(&(b - current.y).abs()));
        let Some(y) = y else {
            return Some(current);
        };
        rows.iter()
            .filter(|(block, row)| same_column(block, row))
            .filter(|(block, row)| {
                (block.text_origin.y + row.bounds.y - y).abs() < 0.1
                    && block.index.abs_diff(current_block.index) <= 1
            })
            .flat_map(|(block, row)| {
                row.carets
                    .iter()
                    .map(move |caret| block.global_caret(*caret))
            })
            .min_by(|a, b| {
                (a.x - preferred_x)
                    .abs()
                    .total_cmp(&(b.x - preferred_x).abs())
            })
    }

    pub fn row_edge(&self, byte: ByteOffset, affinity: Affinity, end: bool) -> Option<CaretStop> {
        let current = self.caret(byte, affinity)?;
        let block = self.blocks.iter().find(|b| {
            b.display_range.start <= byte
                && byte <= b.display_range.end
                && current.y >= b.bounds.y
                && current.y < b.bounds.bottom()
        })?;
        Some(block.global_caret(block.shaped.row_edge(
            ByteOffset(byte.0 - block.display_range.start.0),
            affinity,
            end,
        )))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ShapeKey {
    id: LayoutBlockId,
    generation: u64,
    content: u64,
    geometry: u64,
}
struct Entry {
    block: LayoutBlock,
    content: u64,
    key: ShapeKey,
    measured: bool,
}
struct Cached {
    document: Arc<ShapedDocument>,
    height: f32,
    bytes: usize,
    tick: u64,
}

pub struct VirtualDocument {
    config: LayoutConfig,
    entries: Vec<Entry>,
    by_id: HashMap<LayoutBlockId, usize>,
    heights: HeightIndex,
    cache: HashMap<ShapeKey, Cached>,
    cache_bytes: usize,
    tick: u64,
    evicted: usize,
    measured_count: usize,
    engine_generation: Option<crate::ShapingGeneration>,
}
impl Default for VirtualDocument {
    fn default() -> Self {
        Self::new(LayoutConfig::default()).expect("default layout configuration")
    }
}
impl VirtualDocument {
    pub fn new(config: LayoutConfig) -> Result<Self, LayoutError> {
        config.validate()?;
        Ok(Self {
            config,
            entries: Vec::new(),
            by_id: HashMap::new(),
            heights: HeightIndex::default(),
            cache: HashMap::new(),
            cache_bytes: 0,
            tick: 0,
            evicted: 0,
            measured_count: 0,
            engine_generation: None,
        })
    }
    pub fn config(&self) -> LayoutConfig {
        self.config
    }
    pub fn heights(&self) -> &HeightIndex {
        &self.heights
    }
    pub fn block_count(&self) -> usize {
        self.entries.len()
    }
    pub fn block(&self, index: usize) -> Option<&LayoutBlock> {
        self.entries.get(index).map(|entry| &entry.block)
    }
    pub fn cache_entries(&self) -> usize {
        self.cache.len()
    }
    pub fn cache_bytes(&self) -> usize {
        self.cache_bytes
    }
    pub fn is_block_cached(&self, index: usize) -> bool {
        self.entries
            .get(index)
            .is_some_and(|entry| self.cache.contains_key(&entry.key))
    }
    pub fn prepare_shaper(&mut self, engine: &ShapingEngine) {
        self.synchronize_engine(engine);
    }
    fn synchronize_engine(&mut self, engine: &ShapingEngine) {
        let generation = engine.cache_generation();
        if self.engine_generation == Some(generation) {
            return;
        }
        if self.engine_generation.is_some() {
            self.cache.clear();
            self.cache_bytes = 0;
            for entry in &mut self.entries {
                entry.measured = false;
            }
            self.measured_count = 0;
            self.heights = HeightIndex::new(
                self.entries
                    .iter()
                    .map(|entry| estimate_height(&entry.block, self.config.width)),
            )
            .expect("previously validated block estimates");
        }
        self.engine_generation = Some(generation);
    }

    /// Validation is atomic. Unchanged stable IDs retain exact measured heights
    /// and cache entries even when prefix edits move their global byte ranges.
    pub fn set_blocks(
        &mut self,
        blocks: Vec<LayoutBlock>,
        config: LayoutConfig,
    ) -> Result<(), LayoutError> {
        config.validate()?;
        let mut ids = HashSet::new();
        let mut display_end = ByteOffset(0);
        let mut source_start = ByteOffset(0);
        for block in &blocks {
            block.validate()?;
            if !ids.insert(block.id) {
                return Err(LayoutError::DuplicateBlock(block.id));
            }
            if block.display_range.start < display_end || block.source_range.start < source_start {
                return Err(LayoutError::UnorderedBlocks);
            }
            display_end = block.display_range.end;
            source_start = block.source_range.start;
        }
        let previous: HashMap<_, _> = self
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                (
                    entry.key,
                    (self.heights.height(index).unwrap(), entry.measured),
                )
            })
            .collect();
        let mut heights = Vec::with_capacity(blocks.len());
        let mut entries = Vec::with_capacity(blocks.len());
        let mut by_id = HashMap::new();
        for block in blocks {
            let content = content_key(&block);
            let key = shape_key(&block, content, config);
            let (height, measured) = previous
                .get(&key)
                .copied()
                .or_else(|| self.cache.get(&key).map(|cache| (cache.height, true)))
                .unwrap_or_else(|| (estimate_height(&block, config.width), false));
            by_id.insert(block.id, entries.len());
            heights.push(height);
            entries.push(Entry {
                block,
                key,
                content,
                measured,
            });
        }
        self.heights = HeightIndex::new(heights)?;
        self.entries = entries;
        self.measured_count = self.entries.iter().filter(|entry| entry.measured).count();
        self.by_id = by_id;
        self.config = config;
        self.trim_cache();
        Ok(())
    }

    pub fn configure(&mut self, config: LayoutConfig) -> Result<(), LayoutError> {
        config.validate()?;
        if config == self.config {
            return Ok(());
        }
        if config.width == self.config.width
            && config.scale == self.config.scale
            && config.font_generation == self.config.font_generation
            && config.typography_generation == self.config.typography_generation
        {
            self.config = config;
            self.trim_cache();
            return Ok(());
        }
        let mut updates = Vec::with_capacity(self.entries.len());
        let mut heights = Vec::with_capacity(self.entries.len());
        for (index, entry) in self.entries.iter().enumerate() {
            let key = shape_key(&entry.block, entry.content, config);
            if entry.key != key {
                let measured = self.cache.contains_key(&key);
                let height = self.cache.get(&key).map_or_else(
                    || estimate_height(&entry.block, config.width),
                    |cache| cache.height,
                );
                heights.push(height);
                updates.push((key, measured));
            } else {
                heights.push(self.heights.height(index).unwrap());
                updates.push((key, entry.measured));
            }
        }
        let heights = HeightIndex::new(heights)?;
        for (entry, (key, measured)) in self.entries.iter_mut().zip(updates) {
            entry.key = key;
            entry.measured = measured;
        }
        self.heights = heights;
        self.measured_count = self.entries.iter().filter(|entry| entry.measured).count();
        self.config = config;
        self.trim_cache();
        Ok(())
    }

    pub fn block_for_display(&self, byte: ByteOffset, affinity: Affinity) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        let mut index = self.entries.partition_point(|entry| {
            entry.block.display_range.end < byte
                || (entry.block.display_range.end == byte && affinity == Affinity::After)
        });
        index = index.min(self.entries.len() - 1);
        (self.entries[index].block.display_range.start <= byte
            && byte <= self.entries[index].block.display_range.end)
            .then_some(index)
    }

    pub fn capture_anchor(&self, top: f32) -> Option<BlockScrollAnchor> {
        let index = self.heights.block_at_y(top)?;
        let entry = &self.entries[index];
        let origin = text_origin(entry.block.style, self.heights.prefix(index));
        let (local_display_byte, y) = self
            .cache
            .get(&entry.key)
            .map(|cache| {
                let caret = cache.document.hit_test(Point {
                    x: 0.0,
                    y: top - origin.y,
                });
                (caret.byte, origin.y + caret.y)
            })
            .unwrap_or((ByteOffset(0), self.heights.prefix(index)));
        Some(BlockScrollAnchor {
            block_id: entry.block.id,
            source_byte: entry.block.source_range.start,
            local_display_byte,
            offset_y: top - y,
        })
    }

    fn anchor_index(&self, anchor: BlockScrollAnchor) -> Option<usize> {
        self.by_id.get(&anchor.block_id).copied().or_else(|| {
            if self.entries.is_empty() {
                None
            } else {
                Some(
                    self.entries
                        .partition_point(|entry| entry.block.source_range.end <= anchor.source_byte)
                        .min(self.entries.len() - 1),
                )
            }
        })
    }
    fn available_anchor_top(
        &self,
        anchor: BlockScrollAnchor,
        frame: &HashMap<ShapeKey, Arc<ShapedDocument>>,
    ) -> f32 {
        let Some(index) = self.anchor_index(anchor) else {
            return 0.0;
        };
        let entry = &self.entries[index];
        let document = frame
            .get(&entry.key)
            .or_else(|| self.cache.get(&entry.key).map(|cache| &cache.document));
        let local_y =
            document.map(|document| document.caret(anchor.local_display_byte, Affinity::After).y);
        (local_y.map_or(self.heights.prefix(index), |y| {
            text_origin(entry.block.style, self.heights.prefix(index)).y + y
        }) + anchor.offset_y)
            .max(0.0)
    }
    /// Explicit offscreen resolution. A Defer-policy giant block returns an error
    /// instead of blocking the UI or returning invented caret geometry.
    pub fn resolve_anchor(
        &mut self,
        engine: &mut ShapingEngine,
        anchor: BlockScrollAnchor,
    ) -> Result<f32, LayoutError> {
        let index = self.anchor_index(anchor).ok_or(LayoutError::MissingBlock)?;
        self.ensure_block(engine, index)?;
        Ok(self.available_anchor_top(anchor, &HashMap::new()))
    }

    pub fn ensure_display_byte(
        &mut self,
        engine: &mut ShapingEngine,
        byte: ByteOffset,
        affinity: Affinity,
    ) -> Result<VisibleBlock, LayoutError> {
        let index = self
            .block_for_display(byte, affinity)
            .ok_or(LayoutError::MissingBlock)?;
        let shaped = self.ensure_block(engine, index)?;
        Ok(self.visible_block(index, shaped))
    }

    pub fn ensure_block_at(
        &mut self,
        engine: &mut ShapingEngine,
        index: usize,
    ) -> Result<VisibleBlock, LayoutError> {
        let shaped = self.ensure_block(engine, index)?;
        Ok(self.visible_block(index, shaped))
    }

    /// Resolve a remote caret's block and its immediate neighbors. Hosts use
    /// this for document-end and cross-block arrow navigation before scrolling;
    /// no nearest-visible-caret substitution is made for missing geometry.
    pub fn layout_around_display(
        &mut self,
        engine: &mut ShapingEngine,
        byte: ByteOffset,
        affinity: Affinity,
    ) -> Result<ViewportLayout, LayoutError> {
        let at = self
            .block_for_display(byte, affinity)
            .ok_or(LayoutError::MissingBlock)?;
        let mut blocks = Vec::new();
        let mut unmeasured = Vec::new();
        let mut stats = VirtualLayoutStats {
            total_blocks: self.entries.len(),
            ..Default::default()
        };
        for index in at.saturating_sub(1)..(at + 2).min(self.entries.len()) {
            stats.blocks_considered += 1;
            let cached = self.cache.contains_key(&self.entries[index].key);
            match self.ensure_block_at(engine, index) {
                Ok(block) => {
                    if cached {
                        stats.cache_hits += 1;
                    } else {
                        stats.shaped_blocks += 1;
                        let bytes = self.entries[index].block.content().len();
                        stats.largest_shaped_block_bytes =
                            stats.largest_shaped_block_bytes.max(bytes);
                        if bytes > self.config.long_block_bytes {
                            stats.exact_long_blocks += 1;
                        }
                    }
                    blocks.push(block);
                }
                Err(LayoutError::DeferredLongBlock) if index != at => {
                    unmeasured.push(UnmeasuredBlock {
                        id: self.entries[index].block.id,
                        index,
                        bounds: Rect {
                            x: 0.0,
                            y: self.heights.prefix(index),
                            width: self.config.width,
                            height: self.heights.height(index).unwrap(),
                        },
                        reason: UnmeasuredReason::LongBlock,
                    });
                    stats.deferred_long_blocks += 1;
                }
                Err(error) => return Err(error),
            }
        }
        // Height correction can move the last block, so refresh all origins.
        let blocks = blocks
            .into_iter()
            .map(|block| self.visible_block(block.index, block.shaped))
            .collect::<Vec<_>>();
        let top = blocks.iter().find(|block| block.index == at).map_or(
            self.heights.prefix(at),
            |block| {
                block
                    .global_caret(
                        block
                            .shaped
                            .caret(ByteOffset(byte.0 - block.display_range.start.0), affinity),
                    )
                    .y
            },
        );
        stats.cache_entries = self.cache.len();
        stats.cache_bytes = self.cache_bytes;
        stats.pending_blocks = unmeasured.len();
        Ok(ViewportLayout {
            blocks,
            unmeasured,
            total_height: self.heights.total(),
            height_is_exact: self.measured_count == self.entries.len(),
            corrected_scroll_top: top,
            stats,
        })
    }

    fn ensure_block(
        &mut self,
        engine: &mut ShapingEngine,
        index: usize,
    ) -> Result<Arc<ShapedDocument>, LayoutError> {
        self.synchronize_engine(engine);
        let entry = self.entries.get(index).ok_or(LayoutError::MissingBlock)?;
        self.tick = self.tick.wrapping_add(1);
        if let Some(cached) = self.cache.get_mut(&entry.key) {
            cached.tick = self.tick;
            return Ok(cached.document.clone());
        }
        if entry.block.content().len() > self.config.long_block_bytes
            && self.config.long_block_policy == LongBlockPolicy::Defer
        {
            return Err(LayoutError::DeferredLongBlock);
        }
        let style = entry.block.style;
        let width =
            (self.config.width - style.indent - style.padding.left - style.padding.right).max(1.0);
        let options = LayoutOptions {
            width,
            font_size: style.font_size,
            line_height: style.line_height,
            scale: self.config.scale,
        };
        let mut shaped = if let Some(table) = &entry.block.table {
            engine.shape_table(
                entry.block.content(),
                options,
                style.font,
                &entry.block.spans,
                table,
            )
        } else {
            engine.shape_styled(
                entry.block.content(),
                options,
                style.font,
                &entry.block.spans,
            )
        }
        .map_err(|_| LayoutError::InvalidBlock(entry.block.id))?;
        // Each hard line is a complete layout unit. Its line ending remains in
        // source/display mapping, but the next block owns the following empty row.
        if index + 1 < self.entries.len()
            && shaped.text.ends_with(['\n', '\r'])
            && shaped.rows.len() > 1
            && shaped.rows.last().is_some_and(|row| {
                row.source_range.is_empty() && row.source_range.start.0 == shaped.text.len()
            })
        {
            let last = shaped.rows.pop().unwrap();
            shaped.height = last.bounds.y;
        }
        let height = (shaped.height
            + style.margin_before
            + style.margin_after
            + style.padding.top
            + style.padding.bottom)
            .max(1.0);
        let bytes = derived_bytes(&shaped);
        let document = Arc::new(shaped);
        let key = entry.key;
        self.heights.update(index, height)?;
        if !self.entries[index].measured {
            self.measured_count += 1;
        }
        self.entries[index].measured = true;
        if self.config.max_cached_blocks > 0 && bytes <= self.config.max_cached_bytes {
            self.cache_bytes += bytes;
            self.cache.insert(
                key,
                Cached {
                    document: document.clone(),
                    height,
                    bytes,
                    tick: self.tick,
                },
            );
            self.trim_cache();
        }
        Ok(document)
    }
    fn trim_cache(&mut self) {
        while self.cache.len() > self.config.max_cached_blocks
            || self.cache_bytes > self.config.max_cached_bytes
        {
            let Some(key) = self
                .cache
                .iter()
                .min_by_key(|(_, value)| value.tick)
                .map(|(key, _)| *key)
            else {
                break;
            };
            self.cache_bytes -= self.cache.remove(&key).unwrap().bytes;
            self.evicted += 1;
        }
    }
    fn visible_block(&self, index: usize, shaped: Arc<ShapedDocument>) -> VisibleBlock {
        let entry = &self.entries[index];
        let y = self.heights.prefix(index);
        VisibleBlock {
            id: entry.block.id,
            index,
            source_range: entry.block.source_range.clone(),
            display_range: entry.block.display_range.clone(),
            bounds: Rect {
                x: 0.0,
                y,
                width: self.config.width,
                height: self.heights.height(index).unwrap(),
            },
            text_origin: text_origin(entry.block.style, y),
            shaped,
        }
    }

    pub fn layout_viewport(
        &mut self,
        engine: &mut ShapingEngine,
        viewport: Viewport,
    ) -> ViewportLayout {
        let top = if viewport.top.is_finite() {
            viewport.top.max(0.0)
        } else {
            0.0
        };
        let height = if viewport.height.is_finite() {
            viewport.height.max(0.0)
        } else {
            600.0
        };
        let overscan = if viewport.overscan.is_finite() {
            viewport.overscan.max(0.0)
        } else {
            0.0
        };
        let anchor = self.capture_anchor(top);
        self.synchronize_engine(engine);
        let mut frame = HashMap::new();
        let mut considered = HashSet::new();
        let mut stats = VirtualLayoutStats {
            total_blocks: self.entries.len(),
            ..Default::default()
        };
        let mut corrected = top;
        let mut reasons = HashMap::new();
        let before_eviction = self.evicted;
        while considered.len() < self.config.max_blocks_per_pass {
            if let Some(anchor) = anchor {
                corrected = self.available_anchor_top(anchor, &frame);
            }
            let range = self.heights.intersecting(
                (corrected - overscan).max(0.0),
                corrected + height + overscan,
            );
            let priority = anchor
                .and_then(|anchor| self.anchor_index(anchor))
                .filter(|index| range.contains(index) && !considered.contains(index));
            let Some(index) =
                priority.or_else(|| range.into_iter().find(|index| !considered.contains(index)))
            else {
                break;
            };
            considered.insert(index);
            let entry = &self.entries[index];
            let cached = self.cache.contains_key(&entry.key);
            let bytes = entry.block.content().len();
            match self.ensure_block(engine, index) {
                Ok(document) => {
                    if cached {
                        stats.cache_hits += 1;
                    } else {
                        stats.shaped_blocks += 1;
                        stats.largest_shaped_block_bytes =
                            stats.largest_shaped_block_bytes.max(bytes);
                        if bytes > self.config.long_block_bytes {
                            stats.exact_long_blocks += 1;
                        }
                    }
                    frame.insert(self.entries[index].key, document);
                }
                Err(LayoutError::DeferredLongBlock) => {
                    reasons.insert(index, UnmeasuredReason::LongBlock);
                    stats.deferred_long_blocks += 1;
                }
                Err(_) => {
                    reasons.insert(index, UnmeasuredReason::PassBudget);
                }
            }
        }
        if let Some(anchor) = anchor {
            corrected = self.available_anchor_top(anchor, &frame);
        }
        let range = self.heights.intersecting(
            (corrected - overscan).max(0.0),
            corrected + height + overscan,
        );
        let mut blocks = Vec::new();
        let mut unmeasured = Vec::new();
        for index in range {
            let entry = &self.entries[index];
            if let Some(document) = frame
                .get(&entry.key)
                .or_else(|| self.cache.get(&entry.key).map(|cache| &cache.document))
            {
                blocks.push(self.visible_block(index, document.clone()));
            } else {
                unmeasured.push(UnmeasuredBlock {
                    id: entry.block.id,
                    index,
                    bounds: Rect {
                        x: 0.0,
                        y: self.heights.prefix(index),
                        width: self.config.width,
                        height: self.heights.height(index).unwrap(),
                    },
                    reason: reasons
                        .get(&index)
                        .copied()
                        .unwrap_or(UnmeasuredReason::PassBudget),
                });
            }
        }
        stats.blocks_considered = considered.len();
        stats.pending_blocks = unmeasured.len();
        stats.cache_entries = self.cache.len();
        stats.cache_bytes = self.cache_bytes;
        stats.evicted_blocks = self.evicted - before_eviction;
        ViewportLayout {
            blocks,
            unmeasured,
            total_height: self.heights.total(),
            height_is_exact: self.measured_count == self.entries.len(),
            corrected_scroll_top: corrected,
            stats,
        }
    }
}

fn text_origin(style: BlockStyle, y: f32) -> Point {
    Point {
        x: style.indent + style.padding.left,
        y: y + style.margin_before + style.padding.top,
    }
}
fn vertical_distance(bounds: Rect, y: f32) -> f32 {
    if y < bounds.y {
        bounds.y - y
    } else if y > bounds.bottom() {
        y - bounds.bottom()
    } else {
        0.0
    }
}
fn content_key(block: &LayoutBlock) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    block.content().hash(&mut h);
    block.spans.hash(&mut h);
    block.table.hash(&mut h);
    h.finish()
}
fn shape_key(block: &LayoutBlock, content: u64, config: LayoutConfig) -> ShapeKey {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let s = block.style;
    s.font.hash(&mut h);
    for value in [
        config.width,
        config.scale,
        s.font_size,
        s.line_height,
        s.indent,
        s.margin_before,
        s.margin_after,
        s.padding.left,
        s.padding.right,
        s.padding.top,
        s.padding.bottom,
    ] {
        value.to_bits().hash(&mut h);
    }
    config.font_generation.hash(&mut h);
    config.typography_generation.hash(&mut h);
    ShapeKey {
        id: block.id,
        generation: block.generation,
        content,
        geometry: h.finish(),
    }
}
fn estimate_height(block: &LayoutBlock, width: f32) -> f32 {
    let style = block.style;
    let width = (width - style.indent - style.padding.left - style.padding.right).max(1.0);
    // Estimate only; exact wrapping belongs solely to the shaper.
    let columns = (width / (style.font_size * 0.6)).max(1.0);
    let lines = block
        .content()
        .split_inclusive('\n')
        .map(|line| {
            let chars = line.trim_end_matches(['\n', '\r']).chars().count().max(1);
            (chars as f32 / columns).ceil().max(1.0)
        })
        .sum::<f32>()
        .max(1.0);
    lines * style.line_height
        + style.margin_before
        + style.margin_after
        + style.padding.top
        + style.padding.bottom
}
fn derived_bytes(document: &ShapedDocument) -> usize {
    let mut images = HashSet::new();
    document.text.capacity()
        + document.table_cells.capacity() * std::mem::size_of::<crate::TableCellGeometry>()
        + document.rows.capacity() * std::mem::size_of::<crate::ShapedRow>()
        + document
            .rows
            .iter()
            .map(|row| {
                row.carets.capacity() * std::mem::size_of::<CaretStop>()
                    + row.clusters.capacity() * std::mem::size_of::<crate::GlyphCluster>()
            })
            .sum::<usize>()
        + document.glyphs.capacity() * std::mem::size_of::<crate::PositionedGlyph>()
        + document
            .glyphs
            .iter()
            .filter(|glyph| images.insert(glyph.image.id))
            .map(|glyph| glyph.image.rgba.capacity())
            .sum::<usize>()
}

/// Original Source-line reconciler. Scanning/reconciliation is O(document bytes),
/// while visible shaping is bounded separately. It never pretends that the line
/// scan is incremental CommonMark parsing. Prefix/suffix lines retain identity.
#[derive(Default)]
pub struct SourceDocumentBlocks {
    next_id: u64,
    previous: Vec<LayoutBlock>,
}
impl SourceDocumentBlocks {
    pub fn update(&mut self, text: Arc<str>, style: BlockStyle) -> Vec<LayoutBlock> {
        let ranges = hard_line_ranges(&text);
        let old = &self.previous;
        let mut prefix = 0;
        while prefix < old.len().min(ranges.len())
            && old[prefix].content() == &text[ranges[prefix].clone()]
        {
            prefix += 1;
        }
        let mut suffix = 0;
        while suffix
            < old
                .len()
                .saturating_sub(prefix)
                .min(ranges.len().saturating_sub(prefix))
            && old[old.len() - suffix - 1].content()
                == &text[ranges[ranges.len() - suffix - 1].clone()]
        {
            suffix += 1;
        }
        let mut result = Vec::with_capacity(ranges.len());
        for (index, range) in ranges.into_iter().enumerate() {
            let id = if index < prefix {
                old[index].id
            } else {
                self.next_id = self.next_id.wrapping_add(1).max(1);
                LayoutBlockId(self.next_id)
            };
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            text[range.clone()].hash(&mut hasher);
            result.push(LayoutBlock {
                id,
                generation: hasher.finish(),
                source_range: ByteOffset(range.start)..ByteOffset(range.end),
                display_range: ByteOffset(range.start)..ByteOffset(range.end),
                text: text.clone(),
                text_range: range,
                style,
                spans: Arc::from([]),
                table: None,
            });
        }
        // Restore unchanged suffix identities and retain the identity of one
        // edited line when both sides delimit exactly one replacement line.
        let new_len = result.len();
        for offset in 0..suffix {
            result[new_len - offset - 1].id = old[old.len() - offset - 1].id;
        }
        if old.len().saturating_sub(prefix + suffix) == 1
            && new_len.saturating_sub(prefix + suffix) == 1
        {
            result[prefix].id = old[prefix].id;
        }
        self.previous = result.clone();
        result
    }
}

// Keep a final empty caret line and preserve CRLF as one source ending.
pub fn hard_line_ranges(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut at = 0;
    while at < bytes.len() {
        if matches!(bytes[at], b'\r' | b'\n') {
            let was_cr = bytes[at] == b'\r';
            at += 1;
            if was_cr && bytes.get(at) == Some(&b'\n') {
                at += 1;
            }
            ranges.push(start..at);
            start = at;
        } else {
            at += 1;
        }
    }
    ranges.push(start..text.len());
    ranges
}
