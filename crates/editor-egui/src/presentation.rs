//! Original Source/Rich presentation orchestration over one canonical editor.
//! Rich is caller-gated experimental UI. Derived strings are not editable stores.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    hash::{Hash, Hasher},
    ops::Range,
    sync::{Arc, mpsc::Receiver},
    time::Instant,
};

use egui::{Rect, pos2, vec2};
use glyphfolio_editor_core::{
    Affinity, ByteOffset, CommittedChange, DocumentId, Editor, PresentationMode, Revision,
    Selection,
};
use glyphfolio_editor_layout::{
    BlockStyle, FontFamily, FontStyle, Insets, LayoutBlock, LayoutBlockId, LayoutConfig,
    LongBlockPolicy, ShapingEngine, ShapingGeneration, SourceDocumentBlocks, TextStyleRange,
    Viewport, ViewportLayout, VirtualDocument, hard_line_ranges,
};
use glyphfolio_editor_markdown::{
    BlockContext, BlockKind, DisplayOffset, ListKind, MarkdownParser, ParsedDocument, Projection,
    ProjectionOptions, StyleKind,
};

use crate::geometry::{Geometry, OverlayMap};

#[derive(Clone, Debug, Default)]
pub struct RenderMetrics {
    pub total_blocks: usize,
    pub visible_blocks: usize,
    pub shaped_blocks: usize,
    pub cache_hits: usize,
    pub cache_entries: usize,
    pub cache_bytes: usize,
    pub source_copied_bytes: usize,
    pub projection_copied_bytes: usize,
    pub parsed_bytes: usize,
    pub parse_mode: Option<&'static str>,
    pub layout_microseconds: u64,
    pub accessibility_full_layout: bool,
    pub composition_source_fallback: bool,
    /// Experimental Rich safety fallback for a >32 KiB paragraph containing
    /// more than 512 styles, before invoking attributed whole-paragraph shaping.
    pub large_rich_source_fallback: bool,
    pub exact_long_blocks: usize,
    pub largest_shaped_block_bytes: usize,
    pub reused_geometry: bool,
    /// Rows intersecting the actual paint viewport, excluding remote caret rows.
    pub paint_rows: usize,
    /// Indexed style queries performed for those rows during this preparation.
    pub paint_style_queries: usize,
    pub paint_style_spans: usize,
    pub paint_inline_rects: usize,
}

pub(crate) struct SurfaceInput<'a> {
    pub source_display: &'a str,
    pub source_caret: usize,
    pub preedit_range: Option<Range<usize>>,
    pub selection: Selection,
    /// Effective mode: the host owns the experimental/native gate policy.
    pub mode: PresentationMode,
    pub width: f32,
    pub font_size: f32,
    pub line_height_multiplier: f32,
    pub glyphfolio_typography: bool,
    pub scale: f32,
    pub viewport_top: f32,
    pub viewport_bottom: f32,
    pub accessibility: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum DecorationKind {
    CodeBackground,
    InlineCode,
    QuoteBar,
    Bullet(String),
    Rule,
    TableCell { header: bool, range: Range<usize> },
}
#[derive(Clone, Debug)]
pub(crate) struct Decoration {
    pub bounds: Rect,
    pub kind: DecorationKind,
}
#[derive(Clone, Debug)]
pub(crate) struct PaintSpan {
    pub display_range: Range<usize>,
    pub kind: StyleKind,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedSurface {
    pub images: Vec<ImagePlacement>,
    pub geometry: Geometry,
    pub paint_caret_display: usize,
    pub preedit_display_range: Option<Range<usize>>,
    pub decorations: Vec<Decoration>,
    pub paint_spans: Vec<PaintSpan>,
    pub metrics: RenderMetrics,
    pub corrected_scroll_top: f32,
    pub rendered_mode: PresentationMode,
}

#[derive(Clone, Debug)]
pub(crate) struct ImagePlacement {
    pub bounds: Rect,
    pub destination: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TextKey {
    document: DocumentId,
    revision: Revision,
    mode: PresentationMode,
    reveal: Option<(usize, usize)>,
    overlay: u64,
    font_size: u32,
    line_height_multiplier: u32,
    glyphfolio_typography: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct FrameKey {
    text: TextKey,
    width: u32,
    scale: u32,
    first: usize,
    last: usize,
    caret_block: usize,
    accessibility: bool,
    engine: ShapingGeneration,
}
#[derive(Clone)]
struct BlockMeta {
    kind: BlockKind,
    context: BlockContext,
    revealed: bool,
    first: bool,
    image: Option<(String, egui::Vec2)>,
    task: bool,
}
#[derive(Clone)]
struct Fragment {
    text: String,
    id: LayoutBlockId,
}

pub(crate) struct RenderEngine {
    parser: MarkdownParser,
    parsed: Option<ParsedDocument>,
    changes: Option<Receiver<CommittedChange>>,
    document_id: Option<DocumentId>,
    source_blocks: SourceDocumentBlocks,
    virtual_document: VirtualDocument,
    text_key: Option<TextKey>,
    frame: Option<(FrameKey, PreparedSurface)>,
    display: Arc<str>,
    projection: Option<Arc<Projection>>,
    overlay: Option<OverlayMap>,
    metadata: HashMap<LayoutBlockId, BlockMeta>,
    fragments: HashMap<u64, Vec<Fragment>>,
    next_fragment: u64,
    large_rich_fallback: bool,
    images: HashMap<String, egui::Vec2>,
}
impl Default for RenderEngine {
    fn default() -> Self {
        Self {
            parser: MarkdownParser::with_dialect(
                glyphfolio_editor_markdown::MarkdownDialect::GfmWiki,
            ),
            parsed: None,
            changes: None,
            document_id: None,
            source_blocks: SourceDocumentBlocks::default(),
            virtual_document: VirtualDocument::default(),
            text_key: None,
            frame: None,
            display: Arc::from(""),
            projection: None,
            overlay: None,
            metadata: HashMap::new(),
            fragments: HashMap::new(),
            next_fragment: 1 << 63,
            large_rich_fallback: false,
            images: HashMap::new(),
        }
    }
}

impl RenderEngine {
    pub fn paragraph_after_table(&self, editor: &Editor) -> bool {
        let snapshot = editor.snapshot();
        let at = editor.selection().head;
        editor.selection().is_collapsed()
            && self.parsed.as_ref().is_some_and(|parsed| {
                parsed.document_id() == snapshot.document_id()
                    && parsed.revision() == snapshot.revision()
                    && parsed
                        .blocks()
                        .iter()
                        .any(|block| block.kind == BlockKind::Table && block.source_range.end == at)
            })
    }

    pub fn source_styles(
        &self,
        editor: &Editor,
    ) -> Option<&[glyphfolio_editor_markdown::StyleSpan]> {
        let snapshot = editor.snapshot();
        self.parsed
            .as_ref()
            .filter(|parsed| {
                parsed.document_id() == snapshot.document_id()
                    && parsed.revision() == snapshot.revision()
            })
            .map(|parsed| parsed.styles())
    }

    pub fn heading_at_caret(
        &self,
        editor: &Editor,
    ) -> Option<glyphfolio_editor_core::HeadingLevel> {
        let parsed = self.parsed.as_ref()?;
        let snapshot = editor.snapshot();
        if parsed.document_id() != snapshot.document_id()
            || parsed.revision() != snapshot.revision()
        {
            return None;
        }
        let at = editor.selection().head;
        let level = parsed.blocks().iter().find_map(|block| {
            if block.source_range.start <= at
                && (at < block.source_range.end
                    || at.0 == snapshot.byte_len() && block.source_range.end == at)
            {
                if let BlockKind::Heading(level) = block.kind {
                    Some(level)
                } else {
                    None
                }
            } else {
                None
            }
        })?;
        use glyphfolio_editor_core::HeadingLevel::*;
        match level {
            1 => Some(One),
            2 => Some(Two),
            3 => Some(Three),
            4 => Some(Four),
            5 => Some(Five),
            6 => Some(Six),
            _ => None,
        }
    }
    pub fn command_active(
        &self,
        editor: &Editor,
        command: glyphfolio_editor_core::EditCommand,
    ) -> bool {
        let Some(parsed) = &self.parsed else {
            return false;
        };
        let snapshot = editor.snapshot();
        if parsed.document_id() != snapshot.document_id()
            || parsed.revision() != snapshot.revision()
        {
            return false;
        }
        let selected = editor.selection();
        parsed.styles().iter().any(|span| {
            span.source_range.start <= selected.start()
                && selected.end() <= span.source_range.end
                && (!selected.is_collapsed()
                    || span.source_range.start < selected.head
                        && selected.head < span.source_range.end)
                && matches!(
                    (&span.kind, command),
                    (
                        StyleKind::Strong,
                        glyphfolio_editor_core::EditCommand::ToggleStrong
                    ) | (
                        StyleKind::Emphasis,
                        glyphfolio_editor_core::EditCommand::ToggleEmphasis
                    ) | (
                        StyleKind::Strikethrough,
                        glyphfolio_editor_core::EditCommand::ToggleStrikethrough,
                    ) | (
                        StyleKind::Code,
                        glyphfolio_editor_core::EditCommand::ToggleInlineCode
                    )
                )
        })
    }
    pub fn set_images(&mut self, images: HashMap<String, egui::Vec2>) {
        if self.images != images {
            self.images = images;
            self.text_key = None;
            self.frame = None;
        }
    }

    pub fn prepare(
        &mut self,
        editor: &mut Editor,
        input: SurfaceInput<'_>,
        shaper: &mut ShapingEngine,
    ) -> Result<PreparedSurface, String> {
        let started = Instant::now();
        if !input.width.is_finite()
            || input.width <= 0.0
            || !input.scale.is_finite()
            || input.scale <= 0.0
            || !(1.0..=512.0).contains(&input.font_size)
            || !(1.0..=4.0).contains(&input.line_height_multiplier)
            || !input.viewport_top.is_finite()
            || !input.viewport_bottom.is_finite()
            || input.viewport_bottom < input.viewport_top
            || input.source_caret > input.source_display.len()
            || !input.source_display.is_char_boundary(input.source_caret)
        {
            return Err("Invalid presentation dimensions/caret".into());
        }
        let snapshot = editor.snapshot();
        if self.document_id != Some(snapshot.document_id()) {
            let images = std::mem::take(&mut self.images);
            *self = Self::default();
            self.images = images;
            self.document_id = Some(snapshot.document_id());
            self.changes = Some(editor.subscribe());
        }
        let mut last_change = None;
        let mut change_maps = Vec::new();
        let mut change_count = 0;
        let mut disconnected = false;
        if let Some(receiver) = &self.changes {
            loop {
                match receiver.try_recv() {
                    Ok(change) => {
                        change_count += 1;
                        change_maps.push((
                            change.before_revision,
                            change.revision,
                            change.change_map.clone(),
                        ));
                        last_change = Some(change);
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        if disconnected {
            self.changes = Some(editor.subscribe());
        }
        let mut metrics = RenderMetrics::default();
        let composition_fallback =
            input.mode == PresentationMode::Rich && input.preedit_range.is_some();
        let mut mode = if composition_fallback {
            PresentationMode::Source
        } else {
            input.mode
        };
        metrics.composition_source_fallback = composition_fallback;
        let mut reveal = None;
        if mode == PresentationMode::Rich {
            let current = self.parsed.as_ref().is_some_and(|parsed| {
                parsed.document_id() == snapshot.document_id()
                    && parsed.revision() == snapshot.revision()
            });
            if !current {
                let parsed = if change_count == 1
                    && !disconnected
                    && last_change.as_ref().is_some_and(|change| {
                        self.parsed.as_ref().is_some_and(|old| {
                            old.revision() == change.before_revision
                                && change.revision == snapshot.revision()
                        })
                    }) {
                    self.parser.parse_change(last_change.as_ref().unwrap())
                } else {
                    self.parser.parse(&snapshot)
                };
                metrics.source_copied_bytes = snapshot.byte_len();
                metrics.parsed_bytes = parsed.stats().parsed_bytes;
                metrics.parse_mode = Some(match parsed.stats().mode {
                    glyphfolio_editor_markdown::ParseMode::Full => "full",
                    glyphfolio_editor_markdown::ParseMode::IncrementalParagraph => {
                        "incremental paragraph"
                    }
                    glyphfolio_editor_markdown::ParseMode::Unchanged => "unchanged",
                });
                self.large_rich_fallback = pathological_rich_paragraph(&parsed);
                self.parsed = Some(parsed);
            }
            if self.large_rich_fallback {
                metrics.large_rich_source_fallback = true;
                mode = PresentationMode::Source;
                reveal = None;
            }
        }
        let overlay_fingerprint = if let Some(range) = &input.preedit_range {
            if range.start > range.end
                || range.end > input.source_display.len()
                || !input.source_display.is_char_boundary(range.start)
                || !input.source_display.is_char_boundary(range.end)
            {
                return Err("Invalid provisional display range".into());
            }
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            range.hash(&mut hash);
            input.source_display[range.clone()].hash(&mut hash);
            input.source_display.len().hash(&mut hash);
            hash.finish()
        } else {
            0
        };
        let text_key = TextKey {
            document: snapshot.document_id(),
            revision: snapshot.revision(),
            mode,
            reveal,
            overlay: overlay_fingerprint,
            font_size: input.font_size.to_bits(),
            line_height_multiplier: input.line_height_multiplier.to_bits(),
            glyphfolio_typography: input.glyphfolio_typography,
        };
        let config = LayoutConfig {
            width: input.width,
            scale: input.scale,
            long_block_policy: LongBlockPolicy::Exact,
            ..Default::default()
        };
        // A source position survives presentation and width changes; block IDs
        // belong to an individual projection and cannot bridge Source/Rich.
        let anchor = self.frame.as_ref().and_then(|(old_key, old)| {
            let changed = old_key.text != text_key
                || old_key.width != input.width.to_bits()
                || old_key.scale != input.scale.to_bits()
                || old_key.engine != shaper.cache_generation();
            if !changed {
                return None;
            }
            let caret = old.geometry.hit_test_caret(pos2(0.0, input.viewport_top));
            if (caret.y - input.viewport_top).abs() > 128.0 {
                return None;
            }
            let mut source = caret.byte;
            let mut revision = old_key.text.revision;
            for (before, after, map) in &change_maps {
                if revision == *before {
                    source = map.map_offset(source, Affinity::After).ok()?;
                    revision = *after;
                }
            }
            if revision != snapshot.revision() {
                return None;
            }
            Some((source, input.viewport_top - caret.y))
        });
        if self.text_key.as_ref() != Some(&text_key) {
            if self.text_key.as_ref().is_some_and(|old| old.mode != mode) {
                self.virtual_document = VirtualDocument::default();
            }
            self.metadata.clear();
            self.projection = None;
            self.overlay = None;
            let blocks = if mode == PresentationMode::Rich {
                let parsed = self.parsed.as_ref().unwrap();
                let projected = Arc::new(parsed.project(ProjectionOptions {
                    reveal: reveal.map(|(start, end)| ByteOffset(start)..ByteOffset(end)),
                }));
                self.display = Arc::from(projected.text());
                metrics.projection_copied_bytes += self.display.len();
                let blocks = self.rich_blocks(
                    &projected,
                    input.font_size,
                    input.line_height_multiplier,
                    input.glyphfolio_typography,
                )?;
                self.projection = Some(projected);
                blocks
            } else {
                self.display = Arc::from(input.source_display);
                metrics.source_copied_bytes += input.source_display.len();
                if let Some(range) = &input.preedit_range {
                    let canonical = snapshot.text();
                    metrics.source_copied_bytes += canonical.len();
                    let removed = canonical
                        .len()
                        .checked_add(range.len())
                        .and_then(|len| len.checked_sub(input.source_display.len()))
                        .ok_or("Invalid provisional replacement length")?;
                    let end = range
                        .start
                        .checked_add(removed)
                        .ok_or("Invalid provisional replacement length")?;
                    if end > canonical.len()
                        || !canonical.is_char_boundary(range.start)
                        || !canonical.is_char_boundary(end)
                        || canonical[..range.start] != input.source_display[..range.start]
                        || canonical[end..] != input.source_display[range.end..]
                    {
                        return Err(
                            "Provisional display does not belong to the canonical snapshot".into(),
                        );
                    }
                    self.overlay = Some(OverlayMap {
                        replaced: range.start..end,
                        displayed: range.clone(),
                    });
                }
                let mut style = body_style(input.font_size, input.line_height_multiplier);
                if input.glyphfolio_typography {
                    style.font.family = FontFamily::Monospace;
                }
                self.source_blocks.update(self.display.clone(), style)
            };
            self.virtual_document
                .set_blocks(blocks, config)
                .map_err(|e| e.to_string())?;
            self.text_key = Some(text_key.clone());
            self.frame = None;
        } else {
            self.virtual_document
                .configure(config)
                .map_err(|e| e.to_string())?;
        }
        let paint_caret = self
            .projection
            .as_ref()
            .map_or(Ok(input.source_caret), |p| {
                p.source_to_display(input.selection.head, input.selection.affinity)
                    .map(|at| at.0)
            })
            .map_err(|e| e.to_string())?;
        self.virtual_document.prepare_shaper(shaper);
        let mut viewport_top = input.viewport_top.max(0.0);
        let mut anchor_shaped = 0;
        let mut anchor_largest = 0;
        if let Some((source, pixel_offset)) = anchor {
            let byte = self
                .projection
                .as_ref()
                .map_or(Ok(DisplayOffset(source.0)), |p| {
                    p.source_to_display(source, Affinity::After)
                })
                .map_err(|e| e.to_string())?
                .0;
            let byte = self.overlay.as_ref().map_or(byte, |overlay| {
                crate::geometry::map_replacement(
                    byte,
                    &overlay.replaced,
                    &overlay.displayed,
                    Affinity::After,
                )
            });
            let index = self
                .virtual_document
                .block_for_display(ByteOffset(byte), Affinity::After)
                .ok_or("Missing scroll anchor block")?;
            if !self.virtual_document.is_block_cached(index) {
                anchor_shaped = 1;
                anchor_largest = self
                    .virtual_document
                    .block(index)
                    .map_or(0, |block| block.content().len());
            }
            let block = self
                .virtual_document
                .ensure_display_byte(shaper, ByteOffset(byte), Affinity::After)
                .map_err(|e| e.to_string())?;
            let local = ByteOffset(byte.saturating_sub(block.display_range.start.0));
            viewport_top = (block
                .global_caret(block.shaped.caret(local, Affinity::After))
                .y
                + pixel_offset)
                .max(0.0);
        }
        let viewport_height = (input.viewport_bottom - input.viewport_top).max(1.0);
        let caret_block = self
            .virtual_document
            .block_for_display(ByteOffset(paint_caret), input.selection.affinity)
            .unwrap_or(0);
        let window = self.virtual_document.heights().intersecting(
            (viewport_top - 56.0).max(0.0),
            viewport_top + viewport_height + 56.0,
        );
        let mut frame_key = FrameKey {
            text: text_key,
            width: input.width.to_bits(),
            scale: input.scale.to_bits(),
            first: if input.accessibility { 0 } else { window.start },
            last: if input.accessibility {
                self.virtual_document.block_count()
            } else {
                window.end
            },
            caret_block: if input.accessibility { 0 } else { caret_block },
            accessibility: input.accessibility,
            engine: shaper.cache_generation(),
        };
        if let Some((old, prepared)) = &self.frame
            && *old == frame_key
        {
            let mut prepared = prepared.clone();
            prepared.paint_caret_display = paint_caret;
            prepared.preedit_display_range = input.preedit_range;
            prepared.corrected_scroll_top = viewport_top;
            prepared.metrics.shaped_blocks = 0;
            prepared.metrics.cache_hits = prepared.metrics.visible_blocks;
            prepared.metrics.source_copied_bytes = metrics.source_copied_bytes;
            prepared.metrics.projection_copied_bytes = 0;
            prepared.metrics.parsed_bytes = metrics.parsed_bytes;
            prepared.metrics.parse_mode = metrics.parse_mode;
            prepared.metrics.composition_source_fallback = metrics.composition_source_fallback;
            prepared.metrics.large_rich_source_fallback = metrics.large_rich_source_fallback;
            prepared.metrics.exact_long_blocks = 0;
            prepared.metrics.largest_shaped_block_bytes = 0;
            prepared.metrics.reused_geometry = true;
            let (spans, inline) = self.inline_paint(
                &prepared.geometry,
                viewport_top,
                viewport_top + viewport_height,
                &mut prepared.metrics,
            );
            prepared.paint_spans = spans;
            prepared
                .decorations
                .retain(|decoration| !matches!(decoration.kind, DecorationKind::InlineCode));
            prepared.decorations.extend(inline);
            prepared.metrics.layout_microseconds = started.elapsed().as_micros() as u64;
            return Ok(prepared);
        }
        let view = if input.accessibility {
            metrics.accessibility_full_layout = true;
            let mut blocks = Vec::new();
            for index in 0..self.virtual_document.block_count() {
                let cached = self.virtual_document.is_block_cached(index);
                let bytes = self
                    .virtual_document
                    .block(index)
                    .map_or(0, |block| block.content().len());
                blocks.push(
                    self.virtual_document
                        .ensure_block_at(shaper, index)
                        .map_err(|e| e.to_string())?,
                );
                if cached {
                    metrics.cache_hits += 1;
                } else {
                    metrics.shaped_blocks += 1;
                    metrics.largest_shaped_block_bytes =
                        metrics.largest_shaped_block_bytes.max(bytes);
                    if bytes > config.long_block_bytes {
                        metrics.exact_long_blocks += 1;
                    }
                }
            }
            ViewportLayout {
                total_height: self.virtual_document.heights().total(),
                height_is_exact: true,
                corrected_scroll_top: viewport_top,
                blocks,
                unmeasured: Vec::new(),
                stats: glyphfolio_editor_layout::VirtualLayoutStats {
                    total_blocks: self.virtual_document.block_count(),
                    shaped_blocks: metrics.shaped_blocks,
                    cache_hits: metrics.cache_hits,
                    exact_long_blocks: metrics.exact_long_blocks,
                    largest_shaped_block_bytes: metrics.largest_shaped_block_bytes,
                    ..Default::default()
                },
            }
        } else {
            // Resolve remote caret neighbors before visible layout; otherwise an
            // offscreen document-end caret must never use nearest-visible data.
            let forced = self
                .virtual_document
                .layout_around_display(shaper, ByteOffset(paint_caret), input.selection.affinity)
                .map_err(|e| e.to_string())?;
            let mut visible = self.virtual_document.layout_viewport(
                shaper,
                Viewport {
                    top: viewport_top,
                    height: viewport_height,
                    overscan: 56.0,
                },
            );
            if !visible.unmeasured.is_empty() {
                return Err("Visible layout budget exhausted; source fallback required".into());
            }
            let ids: HashSet<_> = visible.blocks.iter().map(|block| block.id).collect();
            for block in forced.blocks {
                if !ids.contains(&block.id) {
                    visible.blocks.push(
                        self.virtual_document
                            .ensure_block_at(shaper, block.index)
                            .map_err(|e| e.to_string())?,
                    );
                }
            }
            visible.blocks.sort_by_key(|block| block.index);
            visible.stats.shaped_blocks += forced.stats.shaped_blocks;
            visible.stats.cache_hits += forced.stats.cache_hits;
            visible.stats.exact_long_blocks += forced.stats.exact_long_blocks;
            visible.stats.largest_shaped_block_bytes = visible
                .stats
                .largest_shaped_block_bytes
                .max(forced.stats.largest_shaped_block_bytes);
            visible
        };
        metrics.total_blocks = view.stats.total_blocks;
        metrics.visible_blocks = view.blocks.len();
        metrics.shaped_blocks = view.stats.shaped_blocks;
        metrics.cache_hits = view.stats.cache_hits;
        metrics.cache_entries = self.virtual_document.cache_entries();
        metrics.cache_bytes = self.virtual_document.cache_bytes();
        metrics.exact_long_blocks = view.stats.exact_long_blocks;
        metrics.largest_shaped_block_bytes = view.stats.largest_shaped_block_bytes;
        metrics.shaped_blocks += anchor_shaped;
        metrics.largest_shaped_block_bytes = metrics.largest_shaped_block_bytes.max(anchor_largest);
        if anchor_largest > config.long_block_bytes {
            metrics.exact_long_blocks += 1;
        }
        let corrected_scroll_top = view.corrected_scroll_top;
        if !input.accessibility {
            let final_window = self.virtual_document.heights().intersecting(
                (corrected_scroll_top - 56.0).max(0.0),
                corrected_scroll_top + viewport_height + 56.0,
            );
            frame_key.first = final_window.start;
            frame_key.last = final_window.end;
        }
        let shaped = Arc::new(view.to_shaped_document(self.display.to_string()));
        metrics.projection_copied_bytes += self.display.len();
        let geometry = Geometry::projected(
            shaped,
            self.projection.clone(),
            self.overlay.clone(),
            Some(Arc::new(view.clone())),
        );
        let mut images = Vec::new();
        let mut decorations = Vec::new();
        for block in &view.blocks {
            if let Some(meta) = self.metadata.get(&block.id) {
                for cell in &block.shaped.table_cells {
                    decorations.push(Decoration {
                        bounds: Rect::from_min_size(
                            pos2(
                                block.text_origin.x + cell.bounds.x,
                                block.text_origin.y + cell.bounds.y,
                            ),
                            vec2(cell.bounds.width, cell.bounds.height),
                        ),
                        kind: DecorationKind::TableCell {
                            header: cell.row == 0,
                            range: (block.display_range.start.0 + cell.range.start.0)
                                ..(block.display_range.start.0 + cell.range.end.0),
                        },
                    });
                }
                if let Some((destination, size)) = &meta.image {
                    let scale = (180.0 / size.y)
                        .min((input.width - 24.0).max(1.0) / size.x)
                        .min(1.0);
                    let size = *size * scale;
                    images.push(ImagePlacement {
                        bounds: Rect::from_min_size(
                            pos2(
                                block.text_origin.x,
                                block.bounds.y + block.bounds.height - size.y - 8.0,
                            ),
                            size,
                        ),
                        destination: destination.clone(),
                    });
                }
                if matches!(meta.kind, BlockKind::CodeBlock { .. }) {
                    decorations.push(Decoration {
                        bounds: rect(block.bounds),
                        kind: DecorationKind::CodeBackground,
                    });
                }
                if !meta.revealed && meta.context.quote_depth > 0 {
                    decorations.push(Decoration {
                        bounds: Rect::from_min_size(
                            pos2(block.text_origin.x - 10.0, block.bounds.y),
                            vec2(
                                if input.glyphfolio_typography {
                                    3.0
                                } else {
                                    2.0
                                },
                                block.bounds.height,
                            ),
                        ),
                        kind: DecorationKind::QuoteBar,
                    });
                }
                if !meta.revealed
                    && meta.first
                    && meta.context.list_item_start
                    && !meta.task
                    && let Some(kind) = meta.context.list_kind
                {
                    let label = match kind {
                        ListKind::Unordered => "•".into(),
                        ListKind::Ordered { start } => {
                            format!("{}.", meta.context.item_number.unwrap_or(start))
                        }
                    };
                    decorations.push(Decoration {
                        bounds: Rect::from_min_size(
                            pos2(block.text_origin.x - 22.0, block.text_origin.y),
                            vec2(
                                20.0,
                                block
                                    .shaped
                                    .rows
                                    .first()
                                    .map_or(input.font_size, |row| row.bounds.height),
                            ),
                        ),
                        kind: DecorationKind::Bullet(label),
                    });
                }
                if !meta.revealed && matches!(meta.kind, BlockKind::ThematicBreak) {
                    decorations.push(Decoration {
                        bounds: Rect::from_min_size(
                            pos2(
                                block.text_origin.x,
                                block.bounds.y + block.bounds.height * 0.5,
                            ),
                            vec2((input.width - block.text_origin.x).max(0.0), 1.0),
                        ),
                        kind: DecorationKind::Rule,
                    });
                }
            }
        }
        let (paint_spans, inline) = self.inline_paint(
            &geometry,
            corrected_scroll_top,
            corrected_scroll_top + viewport_height,
            &mut metrics,
        );
        decorations.extend(inline);
        metrics.layout_microseconds = started.elapsed().as_micros() as u64;
        let prepared = PreparedSurface {
            images,
            geometry,
            paint_caret_display: paint_caret,
            preedit_display_range: input.preedit_range,
            decorations,
            paint_spans,
            metrics,
            corrected_scroll_top,
            rendered_mode: mode,
        };
        self.frame = Some((frame_key, prepared.clone()));
        Ok(prepared)
    }

    /// A semantic block can contain thousands of hard/soft lines. Query styles
    /// by visible row, and build code backgrounds from those row clusters;
    /// whole-block selection geometry would rescan the complete paragraph for
    /// every inline-code span. Caret-only offscreen rows do not need painting.
    fn inline_paint(
        &self,
        geometry: &Geometry,
        top: f32,
        bottom: f32,
        metrics: &mut RenderMetrics,
    ) -> (Vec<PaintSpan>, Vec<Decoration>) {
        let rows = &geometry.document.rows;
        let first = rows.partition_point(|row| row.bounds.y + row.bounds.height <= top);
        let last = rows.partition_point(|row| row.bounds.y < bottom).max(first);
        metrics.paint_rows = last - first;
        metrics.paint_style_queries = 0;
        metrics.paint_style_spans = 0;
        metrics.paint_inline_rects = 0;
        let mut spans = Vec::new();
        let mut decorations = Vec::new();
        if let Some(projection) = &self.projection {
            for row in &rows[first..last] {
                let block_index = projection
                    .blocks()
                    .partition_point(|block| block.display_range.end.0 <= row.source_range.start.0);
                if projection
                    .blocks()
                    .get(block_index)
                    .is_some_and(|block| matches!(block.kind, BlockKind::CodeBlock { .. }))
                {
                    spans.push(PaintSpan {
                        display_range: row.source_range.start.0..row.source_range.end.0,
                        kind: StyleKind::Code,
                    });
                }
                metrics.paint_style_queries += 1;
                for span in projection.styles_intersecting(
                    DisplayOffset(row.source_range.start.0)..DisplayOffset(row.source_range.end.0),
                ) {
                    let range = span.display_range.start.0.max(row.source_range.start.0)
                        ..span.display_range.end.0.min(row.source_range.end.0);
                    if span.kind == StyleKind::Code {
                        let mut bounds: Option<Rect> = None;
                        for cluster in &row.clusters {
                            if cluster.source_range.start.0 < range.end
                                && range.start < cluster.source_range.end.0
                            {
                                let next = rect(cluster.bounds);
                                bounds = Some(bounds.map_or(next, |bounds| bounds.union(next)));
                            }
                        }
                        if let Some(bounds) = bounds {
                            decorations.push(Decoration {
                                bounds: bounds.expand(1.0),
                                kind: DecorationKind::InlineCode,
                            });
                        }
                    }
                    spans.push(PaintSpan {
                        display_range: range,
                        kind: span.kind.clone(),
                    });
                }
            }
        }
        metrics.paint_style_spans = spans.len();
        metrics.paint_inline_rects = decorations.len();
        (spans, decorations)
    }

    fn rich_blocks(
        &mut self,
        projection: &Projection,
        font_size: f32,
        line_height_multiplier: f32,
        glyphfolio_typography: bool,
    ) -> Result<Vec<LayoutBlock>, String> {
        let mut blocks = Vec::new();
        let mut live = HashSet::new();
        for (position, block) in projection.blocks().iter().enumerate() {
            let mut style = body_style(font_size, line_height_multiplier);
            style.indent =
                block.context.quote_depth as f32 * 14.0 + block.context.list_depth as f32 * 22.0;
            match block.kind {
                BlockKind::Heading(level) => {
                    let index = usize::from(level.saturating_sub(1).min(5));
                    if glyphfolio_typography {
                        let ratios = [
                            26.0 / 16.0,
                            20.0 / 16.0,
                            17.0 / 16.0,
                            15.0 / 16.0,
                            15.0 / 16.0,
                            15.0 / 16.0,
                        ];
                        let line_heights = [1.28, 1.32, 1.36, 1.4, 1.4, 1.4];
                        let weights = [700, 650, 600, 600, 600, 600];
                        style.font_size *= ratios[index];
                        style.line_height = style.font_size * line_heights[index];
                        style.font.weight = weights[index];
                    } else {
                        style.font_size *= [1.85, 1.6, 1.4, 1.25, 1.1, 1.0][index];
                        style.line_height = style.font_size * 1.3;
                        style.font.weight = 700;
                    }
                    style.margin_before = 8.0;
                    style.margin_after = 6.0;
                }
                BlockKind::Table => {
                    style.margin_before = 6.0;
                    style.margin_after = 6.0;
                }
                BlockKind::CodeBlock { .. } => {
                    style.font.family = FontFamily::Monospace;
                    if glyphfolio_typography {
                        style.font_size *= 13.0 / 16.0;
                        style.line_height = style.font_size * 1.5;
                    }
                    let horizontal = if glyphfolio_typography { 14.0 } else { 12.0 };
                    style.padding = Insets {
                        left: horizontal,
                        right: horizontal,
                        top: 8.0,
                        bottom: 8.0,
                    };
                    style.margin_before = 6.0;
                    style.margin_after = 6.0;
                }
                _ => {}
            }
            let image = projection
                .styles_intersecting(block.display_range.clone())
                .find_map(|span| {
                    if let StyleKind::Image { destination } = &span.kind {
                        self.images
                            .get(destination)
                            .map(|size| (destination.clone(), *size))
                    } else {
                        None
                    }
                });
            if let Some((_, size)) = &image {
                let scale = (180.0 / size.y).min(1.0);
                style.padding.bottom += size.y * scale + 16.0;
            }
            let display = block.display_range.start.0..block.display_range.end.0;
            let ranges = if matches!(block.kind, BlockKind::CodeBlock { .. }) {
                let mut ranges = hard_line_ranges(&self.display[display.clone()]);
                if position + 1 < projection.blocks().len()
                    && ranges.last().is_some_and(Range::is_empty)
                    && ranges.len() > 1
                {
                    ranges.pop();
                }
                ranges
                    .into_iter()
                    .map(|range| (display.start + range.start)..(display.start + range.end))
                    .collect::<Vec<_>>()
            } else {
                vec![display.clone()]
            };
            let mut current = Vec::new();
            let previous = self.fragments.get(&block.id.0).cloned().unwrap_or_default();
            let prefix = ranges
                .iter()
                .zip(&previous)
                .take_while(|(range, old)| self.display[(*range).clone()] == old.text)
                .count();
            let mut suffix = 0;
            while suffix
                < ranges
                    .len()
                    .saturating_sub(prefix)
                    .min(previous.len().saturating_sub(prefix))
                && self.display[ranges[ranges.len() - suffix - 1].clone()]
                    == previous[previous.len() - suffix - 1].text
            {
                suffix += 1;
            }
            for (index, range) in ranges.iter().enumerate() {
                let fragmented = matches!(block.kind, BlockKind::CodeBlock { .. });
                let id = if !fragmented {
                    LayoutBlockId(block.id.0)
                } else if index < prefix {
                    previous[index].id
                } else if index >= ranges.len() - suffix {
                    previous[previous.len() - (ranges.len() - index)].id
                } else if ranges.len().saturating_sub(prefix + suffix) == 1
                    && previous.len().saturating_sub(prefix + suffix) == 1
                {
                    previous[prefix].id
                } else {
                    self.next_fragment = self
                        .next_fragment
                        .checked_add(1)
                        .ok_or("Fragment identity exhausted")?;
                    LayoutBlockId(self.next_fragment)
                };
                current.push(Fragment {
                    text: self.display[range.clone()].into(),
                    id,
                });
                let mut fragment_style = style;
                if fragmented {
                    if index > 0 {
                        fragment_style.margin_before = 0.0;
                        fragment_style.padding.top = 0.0;
                    }
                    if index + 1 < ranges.len() {
                        fragment_style.margin_after = 0.0;
                        fragment_style.padding.bottom = 0.0;
                    }
                }
                let source = if !fragmented {
                    block.source_range.clone()
                } else if range.is_empty() {
                    let at = projection
                        .display_to_source(DisplayOffset(range.start), Affinity::After)
                        .map_err(|e| e.to_string())?;
                    at..at
                } else {
                    projection
                        .display_to_source(DisplayOffset(range.start), Affinity::After)
                        .map_err(|e| e.to_string())?
                        ..projection
                            .display_to_source(DisplayOffset(range.end), Affinity::Before)
                            .map_err(|e| e.to_string())?
                };
                let spans = font_spans(projection, range.clone(), style.font);
                let mut hash = std::collections::hash_map::DefaultHasher::new();
                self.display[range.clone()].hash(&mut hash);
                spans.hash(&mut hash);
                let generation = if fragmented {
                    hash.finish()
                } else {
                    block.generation
                };
                let table = if block.kind == BlockKind::Table {
                    glyphfolio_editor_core::MarkdownTable::at(
                        projection.source(),
                        block.source_range.start,
                    )
                    .filter(|table| {
                        table.range == block.source_range
                            && table
                                .rows
                                .iter()
                                .all(|row| row.cells.len() == table.separator.cells.len())
                    })
                    .map(|table| {
                        let rows = table
                            .rows
                            .iter()
                            .map(|row| {
                                row.cells
                                    .iter()
                                    .map(|cell| {
                                        let start = projection
                                            .source_to_display(cell.start, Affinity::After)
                                            .unwrap()
                                            .0
                                            - range.start;
                                        let end = projection
                                            .source_to_display(cell.end, Affinity::Before)
                                            .unwrap()
                                            .0
                                            - range.start;
                                        ByteOffset(start)..ByteOffset(end.max(start))
                                    })
                                    .collect()
                            })
                            .collect();
                        glyphfolio_editor_layout::TableLayoutSpec {
                            rows,
                            alignments: table.alignments(projection.source()),
                        }
                    })
                } else {
                    None
                };
                let mut layout_block = LayoutBlock::from_shared(
                    id,
                    generation,
                    source,
                    ByteOffset(range.start)..ByteOffset(range.end),
                    self.display.clone(),
                    fragment_style,
                )
                .with_spans(spans);
                if let Some(table) = table {
                    layout_block = layout_block.with_table(table);
                }
                blocks.push(layout_block);
                self.metadata.insert(
                    id,
                    BlockMeta {
                        kind: block.kind.clone(),
                        context: block.context.clone(),
                        revealed: block.revealed,
                        first: index == 0,
                        image: if index == 0 { image.clone() } else { None },
                        task: projection
                            .styles_intersecting(block.display_range.clone())
                            .any(|span| matches!(span.kind, StyleKind::TaskCheckbox { .. })),
                    },
                );
            }
            live.insert(block.id.0);
            self.fragments.insert(block.id.0, current);
        }
        self.fragments.retain(|id, _| live.contains(id));
        Ok(blocks)
    }
}

fn body_style(font_size: f32, line_height_multiplier: f32) -> BlockStyle {
    BlockStyle {
        font_size,
        line_height: font_size * line_height_multiplier,
        ..Default::default()
    }
}

fn pathological_rich_paragraph(parsed: &ParsedDocument) -> bool {
    let blocks = parsed.blocks();
    let candidates: HashSet<_> = blocks
        .iter()
        .filter(|block| {
            matches!(block.kind, BlockKind::Paragraph)
                && block.source_range.end.0 - block.source_range.start.0 > 32 * 1024
        })
        .map(|block| block.id)
        .collect();
    if candidates.is_empty() {
        return false;
    }
    let mut counts = HashMap::new();
    for span in parsed.styles() {
        let at = blocks.partition_point(|block| block.source_range.end <= span.source_range.start);
        if let Some(block) = blocks.get(at)
            && candidates.contains(&block.id)
        {
            let count = counts.entry(block.id).or_insert(0usize);
            *count += 1;
            if *count > 512 {
                return true;
            }
        }
    }
    false
}
fn rect(bounds: glyphfolio_editor_layout::Rect) -> Rect {
    Rect::from_min_size(pos2(bounds.x, bounds.y), vec2(bounds.width, bounds.height))
}
fn font_spans(
    projection: &Projection,
    range: Range<usize>,
    base: FontStyle,
) -> Vec<TextStyleRange> {
    // Nested runs are flattened by an event sweep, rather than rechecking
    // every style at every boundary of a potentially enormous paragraph.
    let mut events: BTreeMap<usize, [i32; 3]> = BTreeMap::new();
    events.entry(range.start).or_default();
    events.entry(range.end).or_default();
    for span in projection.styles_intersecting(DisplayOffset(range.start)..DisplayOffset(range.end))
    {
        let kind = match span.kind {
            StyleKind::Strong => 0,
            StyleKind::Emphasis => 1,
            StyleKind::Code => 2,
            _ => continue,
        };
        events
            .entry(span.display_range.start.0.max(range.start))
            .or_default()[kind] += 1;
        events
            .entry(span.display_range.end.0.min(range.end))
            .or_default()[kind] -= 1;
    }
    let mut result: Vec<TextStyleRange> = Vec::new();
    let mut previous = range.start;
    let mut active = [0i32; 3];
    for (position, delta) in events {
        let mut style = base;
        if active[0] > 0 {
            style.weight = 700;
        }
        if active[1] > 0 {
            style.italic = true;
        }
        if active[2] > 0 {
            style.family = FontFamily::Monospace;
        }
        if previous < position && style != base {
            if let Some(last) = result.last_mut()
                && last.style == style
                && last.range.end.0 == previous - range.start
            {
                last.range.end = ByteOffset(position - range.start);
            } else {
                result.push(TextStyleRange {
                    range: ByteOffset(previous - range.start)..ByteOffset(position - range.start),
                    style,
                });
            }
        }
        for kind in 0..3 {
            active[kind] += delta[kind];
        }
        previous = position;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use glyphfolio_editor_core::Transaction;

    fn prepare(
        engine: &mut RenderEngine,
        editor: &mut Editor,
        shaper: &mut ShapingEngine,
        mode: PresentationMode,
        top: f32,
        accessibility: bool,
    ) -> PreparedSurface {
        let source = editor.snapshot().text();
        let selection = editor.selection();
        engine
            .prepare(
                editor,
                SurfaceInput {
                    source_display: &source,
                    source_caret: selection.head.0,
                    preedit_range: None,
                    selection,
                    mode,
                    width: 520.0,
                    font_size: 16.0,
                    line_height_multiplier: 1.45,
                    glyphfolio_typography: false,
                    scale: 1.0,
                    viewport_top: top,
                    viewport_bottom: top + 320.0,
                    accessibility,
                },
                shaper,
            )
            .unwrap()
    }

    #[test]
    fn configured_typography_reflows_without_editing_source_or_history() {
        let source = "alpha beta\nsecond line";
        let mut editor = Editor::new(source);
        let revision = editor.snapshot().revision();
        let selection = editor.selection();
        let mut engine = RenderEngine::default();
        let mut shaper = ShapingEngine::new();
        let configured = |font_size, line_height_multiplier| SurfaceInput {
            source_display: source,
            source_caret: selection.head.0,
            preedit_range: None,
            selection,
            mode: PresentationMode::Source,
            width: 520.0,
            font_size,
            line_height_multiplier,
            glyphfolio_typography: true,
            scale: 1.0,
            viewport_top: 0.0,
            viewport_bottom: 320.0,
            accessibility: false,
        };
        let first = engine
            .prepare(&mut editor, configured(16.0, 1.6), &mut shaper)
            .unwrap();
        let first_height = first.geometry.document.rows[0].bounds.height;
        let first_block = engine.virtual_document.block(0).unwrap().id;
        assert_eq!(
            engine.virtual_document.block(0).unwrap().style.font.family,
            FontFamily::Monospace
        );
        let warm = engine
            .prepare(&mut editor, configured(16.0, 1.6), &mut shaper)
            .unwrap();
        assert!(warm.metrics.reused_geometry);
        let larger = engine
            .prepare(&mut editor, configured(20.0, 2.0), &mut shaper)
            .unwrap();
        assert!(larger.metrics.shaped_blocks > 0);
        assert!(larger.geometry.document.rows[0].bounds.height > first_height * 1.4);
        assert_eq!(engine.virtual_document.block(0).unwrap().id, first_block);
        assert_eq!(larger.geometry.document.text, source);
        assert_eq!(editor.snapshot().revision(), revision);
        assert_eq!(editor.selection(), selection);
        assert_eq!(editor.snapshot().text(), source);
        assert!(!editor.can_undo());
    }

    #[test]
    fn glyphfolio_rich_profile_uses_heading_and_fence_reference_metrics() {
        let source = "# One\n\n## Two\n\n### Three\n\n#### Four\n\n##### Five\n\n###### Six\n\n```rust\nlet value = 42;\n```\n\ntail";
        let mut editor = Editor::new(source);
        editor
            .set_selection(Selection::caret(ByteOffset(source.len())))
            .unwrap();
        let mut engine = RenderEngine::default();
        let mut shaper = ShapingEngine::new();
        let selection = editor.selection();
        engine
            .prepare(
                &mut editor,
                SurfaceInput {
                    source_display: source,
                    source_caret: source.len(),
                    preedit_range: None,
                    selection,
                    mode: PresentationMode::Rich,
                    width: 520.0,
                    font_size: 16.0,
                    line_height_multiplier: 1.6,
                    glyphfolio_typography: true,
                    scale: 1.0,
                    viewport_top: 0.0,
                    viewport_bottom: 900.0,
                    accessibility: false,
                },
                &mut shaper,
            )
            .unwrap();
        let projection = engine.projection.as_ref().unwrap();
        for block in projection.blocks() {
            if let BlockKind::Heading(level) = block.kind {
                let style = engine
                    .virtual_document
                    .block(
                        (0..engine.virtual_document.block_count())
                            .find(|index| {
                                engine.virtual_document.block(*index).unwrap().id
                                    == LayoutBlockId(block.id.0)
                            })
                            .unwrap(),
                    )
                    .unwrap()
                    .style;
                let index = usize::from(level - 1);
                assert_eq!(style.font_size, [26.0, 20.0, 17.0, 15.0, 15.0, 15.0][index]);
                assert_eq!(style.font.weight, [700, 650, 600, 600, 600, 600][index]);
            }
        }
        let code = (0..engine.virtual_document.block_count())
            .filter_map(|index| engine.virtual_document.block(index))
            .find(|block| block.style.font.family == FontFamily::Monospace)
            .unwrap();
        assert_eq!(code.style.font_size, 13.0);
        assert_eq!(code.style.line_height, 19.5);
        assert_eq!(code.style.padding.left, 14.0);
        assert_eq!(editor.snapshot().text(), source);
        assert!(!editor.can_undo());
    }

    #[test]
    fn rich_projection_styles_and_geometry_refer_to_canonical_source() {
        let source = "# Title\n\n**bold** and &amp; [link](https://example.test) `code`\n\nlast";
        let mut editor = Editor::new(source);
        editor
            .set_selection(Selection::caret(ByteOffset(source.len())))
            .unwrap();
        let mut engine = RenderEngine::default();
        let mut shaper = ShapingEngine::new();
        let surface = prepare(
            &mut engine,
            &mut editor,
            &mut shaper,
            PresentationMode::Rich,
            0.0,
            false,
        );
        assert_eq!(
            surface.geometry.document.text,
            "Title\n\nbold and & link code\n\nlast"
        );
        assert!(
            surface
                .paint_spans
                .iter()
                .any(|span| span.kind == StyleKind::Strong)
        );
        assert!(
            surface
                .decorations
                .iter()
                .any(|decoration| matches!(decoration.kind, DecorationKind::InlineCode))
        );
        let offset = source.find("bold").unwrap() + 2;
        let display = surface.geometry.to_display(offset, Affinity::After);
        assert_eq!(
            &surface.geometry.document.text[display - 2..display + 2],
            "bold"
        );
        assert_eq!(surface.geometry.to_source(display, Affinity::After), offset);
        let caret = surface.geometry.caret(ByteOffset(offset), Affinity::After);
        let clicked = surface
            .geometry
            .hit_test_caret(pos2(caret.x, caret.y + caret.height * 0.5));
        assert_eq!(clicked.byte.0, offset);
        let link = surface.geometry.document.text.find("link").unwrap() + 1;
        let caret = surface.geometry.display_caret(link, Affinity::After);
        assert_eq!(
            surface
                .geometry
                .link_at(pos2(caret.x, caret.y + caret.height * 0.5))
                .as_deref(),
            Some("https://example.test")
        );
        assert_eq!(editor.snapshot().text(), source);
        assert!(!editor.can_undo());
    }

    #[test]
    fn source_large_document_only_shapes_visible_and_remote_caret_neighbors() {
        let source: String = (0..10_000)
            .map(|line| format!("line {line:05}: native text\n"))
            .collect();
        let mut editor = Editor::new(&source);
        editor
            .set_selection(Selection::caret(ByteOffset(source.len())))
            .unwrap();
        let mut engine = RenderEngine::default();
        let mut shaper = ShapingEngine::new();
        let first = prepare(
            &mut engine,
            &mut editor,
            &mut shaper,
            PresentationMode::Source,
            0.0,
            false,
        );
        assert_eq!(first.metrics.total_blocks, 10_001);
        assert!(first.metrics.shaped_blocks < 40, "{:?}", first.metrics);
        assert!(first.geometry.document.rows.len() < 40);
        let end = first
            .geometry
            .caret(ByteOffset(source.len()), Affinity::After);
        assert_eq!(end.byte.0, source.len());
        assert!(end.y > 100_000.0);
        let previous = first
            .geometry
            .vertical_target(end.byte, end.affinity, false, end.x);
        assert!(previous.byte < end.byte && previous.y < end.y);
        let second = prepare(
            &mut engine,
            &mut editor,
            &mut shaper,
            PresentationMode::Source,
            0.0,
            false,
        );
        assert!(Arc::ptr_eq(
            &first.geometry.document,
            &second.geometry.document
        ));
        assert!(second.metrics.reused_geometry);
        assert_eq!(second.metrics.shaped_blocks, 0);
        assert!(second.metrics.cache_entries <= 128);
    }

    #[test]
    fn caret_changes_keep_rich_projection_folded_without_reflow_or_history() {
        let source = "**first**\n\n**second**";
        let mut editor = Editor::new(source);
        let mut engine = RenderEngine::default();
        let mut shaper = ShapingEngine::new();
        editor
            .set_selection(Selection::caret(ByteOffset(source.len())))
            .unwrap();
        let last = prepare(
            &mut engine,
            &mut editor,
            &mut shaper,
            PresentationMode::Rich,
            0.0,
            false,
        );
        assert_eq!(last.geometry.document.text, "first\n\nsecond");
        editor
            .set_selection(Selection::caret(ByteOffset(3)))
            .unwrap();
        let first = prepare(
            &mut engine,
            &mut editor,
            &mut shaper,
            PresentationMode::Rich,
            0.0,
            false,
        );
        assert_eq!(first.geometry.document.text, "first\n\nsecond");
        for at in [2, 4, 13, 16] {
            assert_eq!(
                first.geometry.caret_with_affinity(at, Affinity::After),
                last.geometry.caret_with_affinity(at, Affinity::After)
            );
        }
        assert_eq!(first.metrics.shaped_blocks, 0);
        let moved = editor.selection();
        let source_view = prepare(
            &mut engine,
            &mut editor,
            &mut shaper,
            PresentationMode::Source,
            0.0,
            false,
        );
        assert_eq!(source_view.geometry.document.text, source);
        assert_eq!(editor.selection(), moved);
        assert_eq!(editor.snapshot().revision(), Revision(0));
        assert!(!editor.can_undo());
    }

    #[test]
    fn composition_is_disposable_source_overlay_and_keeps_replacement_mapping() {
        let source = "**before**\n\nold selection\n\nafter";
        let mut editor = Editor::new(source);
        let start = source.find("old").unwrap();
        let end = source.find("\n\nafter").unwrap();
        let selection = Selection {
            anchor: ByteOffset(start),
            head: ByteOffset(end),
            affinity: Affinity::After,
        };
        editor.set_selection(selection).unwrap();
        let preedit = "日本語";
        let display = format!("{}{preedit}{}", &source[..start], &source[end..]);
        let mut engine = RenderEngine::default();
        let surface = engine
            .prepare(
                &mut editor,
                SurfaceInput {
                    source_display: &display,
                    source_caret: start + preedit.len(),
                    preedit_range: Some(start..start + preedit.len()),
                    selection,
                    mode: PresentationMode::Rich,
                    width: 520.0,
                    font_size: 16.0,
                    line_height_multiplier: 1.45,
                    glyphfolio_typography: false,
                    scale: 1.0,
                    viewport_top: 0.0,
                    viewport_bottom: 320.0,
                    accessibility: false,
                },
                &mut ShapingEngine::new(),
            )
            .unwrap();
        assert_eq!(surface.rendered_mode, PresentationMode::Source);
        assert!(surface.metrics.composition_source_fallback);
        assert_eq!(surface.geometry.document.text, display);
        assert_eq!(surface.geometry.to_source(start, Affinity::Before), start);
        assert_eq!(
            surface
                .geometry
                .to_source(start + preedit.len(), Affinity::After),
            end
        );
        assert_eq!(
            surface.geometry.to_display(end, Affinity::After),
            start + preedit.len()
        );
        assert_eq!(editor.snapshot().text(), source);
        assert_eq!(editor.selection(), selection);
        assert!(!editor.can_undo());
    }

    #[test]
    fn accessibility_projects_complete_offscreen_text_and_reuses_full_geometry() {
        let source: String = (0..200)
            .map(|line| format!("**line {line}**\n\n"))
            .collect();
        let mut editor = Editor::new(&source);
        editor
            .set_selection(Selection::caret(ByteOffset(source.len())))
            .unwrap();
        let mut engine = RenderEngine::default();
        let mut shaper = ShapingEngine::new();
        let first = prepare(
            &mut engine,
            &mut editor,
            &mut shaper,
            PresentationMode::Rich,
            0.0,
            true,
        );
        assert!(first.metrics.accessibility_full_layout);
        assert!(first.geometry.document.rows.len() >= 400);
        assert!(first.geometry.document.text.contains("line 199"));
        assert!(!first.geometry.document.text.contains("**line 0**"));
        let offscreen = source.find("line 150").unwrap() + 3;
        let display = first.geometry.to_display(offscreen, Affinity::After);
        assert_eq!(
            first.geometry.to_source(display, Affinity::After),
            offscreen
        );
        assert!(
            first
                .geometry
                .caret(ByteOffset(offscreen), Affinity::After)
                .y
                > 1_000.0
        );
        let second = prepare(
            &mut engine,
            &mut editor,
            &mut shaper,
            PresentationMode::Rich,
            2_000.0,
            true,
        );
        assert!(Arc::ptr_eq(
            &first.geometry.document,
            &second.geometry.document
        ));
        assert!(second.metrics.reused_geometry);
    }

    #[test]
    fn long_rich_fence_is_fragmented_at_hard_lines_and_preserves_all_display_text() {
        let body: String = (0..10_000)
            .map(|line| format!("let line_{line} = {line};\n"))
            .collect();
        let source = format!("```rust\n{body}```\n\nafter");
        let mut editor = Editor::new(&source);
        editor
            .set_selection(Selection::caret(ByteOffset(source.len())))
            .unwrap();
        let mut engine = RenderEngine::default();
        let surface = prepare(
            &mut engine,
            &mut editor,
            &mut ShapingEngine::new(),
            PresentationMode::Rich,
            0.0,
            false,
        );
        assert!(surface.metrics.total_blocks >= 10_000);
        assert!(surface.metrics.shaped_blocks < 40, "{:?}", surface.metrics);
        assert_eq!(surface.metrics.exact_long_blocks, 0);
        assert!(surface.geometry.document.text.starts_with(&body));
        assert!(surface.geometry.document.text.ends_with("after"));
        assert!(
            surface
                .decorations
                .iter()
                .any(|decoration| matches!(decoration.kind, DecorationKind::CodeBackground))
        );
        assert_eq!(editor.snapshot().text(), source);
    }

    #[test]
    fn mode_and_prefix_edit_keep_logical_scroll_anchor() {
        let source: String = (0..100)
            .map(|line| format!("## Heading {line}\n\nparagraph **{line}**\n\n"))
            .collect();
        let mut editor = Editor::new(&source);
        let mut engine = RenderEngine::default();
        let mut shaper = ShapingEngine::new();
        let first = prepare(
            &mut engine,
            &mut editor,
            &mut shaper,
            PresentationMode::Source,
            1_000.0,
            false,
        );
        let old = first
            .geometry
            .hit_test_caret(pos2(0.0, first.corrected_scroll_top));
        let delta = first.corrected_scroll_top - old.y;
        let rich = prepare(
            &mut engine,
            &mut editor,
            &mut shaper,
            PresentationMode::Rich,
            first.corrected_scroll_top,
            false,
        );
        let caret = rich.geometry.caret(old.byte, old.affinity);
        assert!((rich.corrected_scroll_top - (caret.y + delta)).abs() < 0.1);
        let before_edit = rich
            .geometry
            .hit_test_caret(pos2(0.0, rich.corrected_scroll_top));
        let delta = rich.corrected_scroll_top - before_edit.y;
        let transaction = Transaction::new(&editor.snapshot())
            .replace(ByteOffset(0)..ByteOffset(0), "prefix\n\n");
        editor.apply(transaction).unwrap();
        let edited = prepare(
            &mut engine,
            &mut editor,
            &mut shaper,
            PresentationMode::Rich,
            rich.corrected_scroll_top,
            false,
        );
        let caret = edited.geometry.caret(
            ByteOffset(before_edit.byte.0 + "prefix\n\n".len()),
            before_edit.affinity,
        );
        assert!(
            (edited.corrected_scroll_top - (caret.y + delta)).abs() < 0.1,
            "{} {}",
            edited.corrected_scroll_top,
            caret.y + delta
        );
        assert!(editor.can_undo());
        editor.undo().unwrap();
        assert_eq!(editor.snapshot().text(), source);
    }

    #[test]
    fn invalid_geometry_or_overlay_returns_error_without_document_mutation() {
        let mut editor = Editor::new("source");
        let mut engine = RenderEngine::default();
        let input = SurfaceInput {
            source_display: "invalid",
            source_caret: 0,
            preedit_range: Some(0..1),
            selection: Selection::default(),
            mode: PresentationMode::Rich,
            width: 0.0,
            font_size: 16.0,
            line_height_multiplier: 1.45,
            glyphfolio_typography: false,
            scale: 1.0,
            viewport_top: 0.0,
            viewport_bottom: 320.0,
            accessibility: false,
        };
        assert!(
            engine
                .prepare(&mut editor, input, &mut ShapingEngine::new())
                .is_err()
        );
        assert_eq!(editor.snapshot().text(), "source");
        assert!(!editor.can_undo());
    }

    #[test]
    fn empty_hidden_and_unfinished_blocks_keep_finite_navigable_geometry() {
        let mut shaper = ShapingEngine::new();
        for source in [
            "",
            "\n",
            "---",
            "#",
            "```\n```",
            "[id]: /target\n",
            ">\n",
            "- ",
            "```rust\nunfinished\n",
        ] {
            let mut editor = Editor::new(source);
            let mut engine = RenderEngine::default();
            for mode in [PresentationMode::Source, PresentationMode::Rich] {
                let surface = prepare(&mut engine, &mut editor, &mut shaper, mode, 0.0, false);
                let caret = surface.geometry.caret(ByteOffset(0), Affinity::After);
                assert!(
                    caret.x.is_finite() && caret.y.is_finite() && caret.height > 0.0,
                    "{source:?}"
                );
                assert!(surface.geometry.document.height.is_finite());
                assert_eq!(editor.snapshot().text(), source);
            }
        }
    }

    #[test]
    fn host_scroll_in_same_window_is_respected_when_geometry_is_reused() {
        let source: String = (0..200).map(|line| format!("line {line}\n")).collect();
        let mut editor = Editor::new(&source);
        let mut engine = RenderEngine::default();
        let mut shaper = ShapingEngine::new();
        let first = prepare(
            &mut engine,
            &mut editor,
            &mut shaper,
            PresentationMode::Source,
            0.0,
            true,
        );
        let second = prepare(
            &mut engine,
            &mut editor,
            &mut shaper,
            PresentationMode::Source,
            10.0,
            true,
        );
        assert!(Arc::ptr_eq(
            &first.geometry.document,
            &second.geometry.document
        ));
        assert_eq!(second.corrected_scroll_top, 10.0);
    }

    #[test]
    fn giant_styled_paragraph_uses_explicit_source_safety_fallback() {
        // CommonMark soft line breaks do not create separate paragraphs. This
        // is guarded before COSMIC's attributed-paragraph worst case. This
        // proves Source safety, not successful Rich paragraph qualification.
        let body: String = (0..10_000)
            .map(|line| format!("line {line:05}: **bold** `code`\n"))
            .collect();
        let source = format!("{body}\nlast");
        let mut editor = Editor::new(&source);
        editor
            .set_selection(Selection::caret(ByteOffset(source.len())))
            .unwrap();
        let mut engine = RenderEngine::default();
        let mut shaper = ShapingEngine::new();
        let first = prepare(
            &mut engine,
            &mut editor,
            &mut shaper,
            PresentationMode::Rich,
            0.0,
            false,
        );
        assert_eq!(first.rendered_mode, PresentationMode::Source);
        assert!(first.metrics.large_rich_source_fallback);
        assert_eq!(first.metrics.exact_long_blocks, 0, "{:?}", first.metrics);
        assert_eq!(first.metrics.total_blocks, 10_002);
        assert!(first.metrics.shaped_blocks < 40);
        assert!(first.geometry.document.rows.len() < 40);
        assert!(first.metrics.paint_rows < 20, "{:?}", first.metrics);
        assert_eq!(first.metrics.paint_style_queries, 0);
        assert_eq!(first.geometry.document.text, source);
        let scrolled = prepare(
            &mut engine,
            &mut editor,
            &mut shaper,
            PresentationMode::Rich,
            0.0,
            false,
        );
        assert!(Arc::ptr_eq(
            &first.geometry.document,
            &scrolled.geometry.document
        ));
        assert_eq!(scrolled.metrics.shaped_blocks, 0);
        assert_eq!(scrolled.metrics.exact_long_blocks, 0);
        assert!(scrolled.metrics.large_rich_source_fallback);
        assert_eq!(editor.snapshot().text(), source);
    }

    #[test]
    fn ten_thousand_style_rows_only_query_and_decorate_the_paint_viewport() {
        // The row geometry is deliberately tiled from one real shaped row to
        // isolate our paint resource contract from unqualified backend shaping.
        let source: String = (0..10_000)
            .map(|line| format!("line {line:05}: **bold** `code`\n"))
            .collect();
        let editor = Editor::new(&source);
        let projection = Arc::new(
            MarkdownParser::new()
                .parse(&editor.snapshot())
                .project(ProjectionOptions::default()),
        );
        let chunk = projection.text().find("line 00001").unwrap();
        let mut shaper = ShapingEngine::new();
        let shaped = shaper.shape(
            &projection.text()[..chunk],
            glyphfolio_editor_layout::LayoutOptions {
                width: 1_000.0,
                font_size: 16.0,
                line_height: 23.2,
                scale: 1.0,
            },
        );
        assert_eq!(shaped.rows.len(), 1);
        let mut rows = Vec::new();
        for index in 0..10_000 {
            let mut row = shaped.rows[0].clone();
            let start = index * chunk;
            let y = index as f32 * 23.2;
            row.source_range = ByteOffset(start)..ByteOffset(start + chunk);
            row.bounds.y += y;
            for caret in &mut row.carets {
                caret.byte.0 += start;
                caret.y += y;
            }
            for cluster in &mut row.clusters {
                cluster.source_range.start.0 += start;
                cluster.source_range.end.0 += start;
                cluster.bounds.y += y;
            }
            rows.push(row);
        }
        let document = Arc::new(glyphfolio_editor_layout::ShapedDocument {
            text: projection.text().into(),
            rows,
            glyphs: Vec::new(),
            width: 1_000.0,
            height: 232_000.0,
            diagnostics: Default::default(),
            table_cells: Vec::new(),
        });
        let geometry = Geometry::projected(document, Some(projection.clone()), None, None);
        let engine = RenderEngine {
            projection: Some(projection),
            ..Default::default()
        };
        let mut metrics = RenderMetrics::default();
        let (first, _) = engine.inline_paint(&geometry, 0.0, 320.0, &mut metrics);
        assert!(metrics.paint_rows < 20);
        assert_eq!(metrics.paint_style_queries, metrics.paint_rows);
        assert!(metrics.paint_style_spans < 40);
        let first_last = first.last().unwrap().display_range.end;
        let (scrolled, decorations) =
            engine.inline_paint(&geometry, 10_000.0, 10_320.0, &mut metrics);
        assert!(metrics.paint_style_queries < 20);
        assert!(scrolled.first().unwrap().display_range.start > first_last);
        assert!(decorations.len() < 20);
        assert!(
            decorations
                .iter()
                .all(|decoration| decoration.bounds.bottom() > 10_000.0
                    && decoration.bounds.top() < 10_320.0)
        );
    }
}

#[cfg(test)]
mod asset_tests {
    use super::*;
    #[test]
    fn native_table_rows_shape_all_grid_separators() {
        let source = include_str!("../../../examples/standalone-egui/fixtures/editing-parity.md");
        let mut editor = Editor::new(source);
        let mut engine = RenderEngine::default();
        let mut shaper = ShapingEngine::new();
        let selection = editor.selection();
        let surface = engine
            .prepare(
                &mut editor,
                SurfaceInput {
                    source_display: source,
                    source_caret: 0,
                    preedit_range: None,
                    selection,
                    mode: PresentationMode::Rich,
                    width: 760.0,
                    font_size: 16.0,
                    line_height_multiplier: 1.6,
                    glyphfolio_typography: true,
                    scale: 1.0,
                    viewport_top: 0.0,
                    viewport_bottom: 900.0,
                    accessibility: false,
                },
                &mut shaper,
            )
            .unwrap();
        let document = &surface.geometry.document;
        assert_eq!(document.table_cells.len(), 6);
        assert!(!document.text.contains('│'));
        let right = document
            .table_cells
            .iter()
            .find(|cell| cell.row == 1 && cell.column == 1)
            .unwrap();
        let hit = surface.geometry.hit_test_caret(pos2(
            right.bounds.x + right.bounds.width - 15.0,
            right.bounds.y + 15.0,
        ));
        let source_cell = glyphfolio_editor_core::MarkdownTable::at(
            source,
            ByteOffset(source.find("Tables").unwrap()),
        )
        .unwrap()
        .rows[1]
            .cells[1]
            .clone();
        assert!(source_cell.start <= hit.byte && hit.byte <= source_cell.end);
    }

    #[test]
    fn resolved_image_reserves_geometry_without_changing_markdown() {
        let source = "![sample](assets/test.png)\n\ntail";
        let mut editor = Editor::new(source);
        editor
            .set_selection(Selection::caret(ByteOffset(source.len())))
            .unwrap();
        let mut engine = RenderEngine::default();
        engine.set_images(HashMap::from([(
            "assets/test.png".into(),
            vec2(80.0, 40.0),
        )]));
        let mut shaper = ShapingEngine::new();
        let selection = editor.selection();
        let result = engine
            .prepare(
                &mut editor,
                SurfaceInput {
                    source_display: source,
                    source_caret: source.len(),
                    preedit_range: None,
                    selection,
                    mode: PresentationMode::Rich,
                    width: 520.0,
                    font_size: 16.0,
                    line_height_multiplier: 1.6,
                    glyphfolio_typography: true,
                    scale: 1.0,
                    viewport_top: 0.0,
                    viewport_bottom: 480.0,
                    accessibility: false,
                },
                &mut shaper,
            )
            .unwrap();
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.images[0].bounds.size(), vec2(80.0, 40.0));
        assert_eq!(editor.snapshot().text(), source);
        assert!(!editor.can_undo());
        engine.set_images(HashMap::new());
    }
}
