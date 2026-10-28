use crate::{Atom, Block, BlockContext, BlockId, BlockKind, ParsedDocument, StyleKind};
use glyphfolio_editor_core::{
    Affinity, ByteOffset, DocumentId, Revision, Selection, Snapshot, Transaction, TransactionOrigin,
};
use std::{
    collections::hash_map::DefaultHasher,
    fmt,
    hash::{Hash, Hasher},
    ops::Range,
    sync::Arc,
};
use unicode_segmentation::UnicodeSegmentation;

/// UTF-8 byte coordinate in the disposable display projection, not Markdown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DisplayOffset(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentKind {
    Copy,
    Hidden,
    Transform,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectionSegment {
    pub source_range: Range<ByteOffset>,
    pub display_range: Range<DisplayOffset>,
    pub kind: SegmentKind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectionOptions {
    /// Reveal all intersecting blocks. A caret range reveals its containing
    /// block; source EOF belongs to the final block. No document mutation occurs.
    pub reveal: Option<Range<ByteOffset>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayStyleSpan {
    pub display_range: Range<DisplayOffset>,
    pub kind: StyleKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectedBlock {
    pub id: BlockId,
    /// Content/style/reveal fingerprint; moving an unchanged block keeps it.
    pub generation: u64,
    pub source_range: Range<ByteOffset>,
    pub display_range: Range<DisplayOffset>,
    pub kind: BlockKind,
    pub context: BlockContext,
    pub revealed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectionError {
    InvalidSourceOffset(ByteOffset),
    InvalidDisplayOffset(DisplayOffset),
    NotGraphemeBoundary(DisplayOffset),
    ReversedRange,
    StaleSnapshot,
    /// Partial edits inside transformed spelling are not guessed. Reveal this
    /// source segment/block or select the whole transform before retrying.
    RevealRequired {
        source_range: Range<ByteOffset>,
    },
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSourceOffset(offset) => {
                write!(f, "invalid Markdown byte offset {}", offset.0)
            }
            Self::InvalidDisplayOffset(offset) => {
                write!(f, "invalid display byte offset {}", offset.0)
            }
            Self::NotGraphemeBoundary(offset) => {
                write!(f, "display offset {} splits a grapheme", offset.0)
            }
            Self::ReversedRange => write!(f, "projection edit range is reversed"),
            Self::StaleSnapshot => write!(f, "projection belongs to another document or revision"),
            Self::RevealRequired { source_range } => write!(
                f,
                "reveal transformed Markdown {}..{} before a partial edit",
                source_range.start.0, source_range.end.0
            ),
        }
    }
}
impl std::error::Error for ProjectionError {}

/// Immutable mapping over one named source snapshot. Hidden syntax gives
/// equivalent boundary positions, not a universal bijection. `Before` chooses
/// the earliest associated boundary, `After` the latest; output carets snap only
/// to legal extended-grapheme stops in their destination coordinate space.
#[derive(Clone, Debug)]
pub struct Projection {
    document_id: DocumentId,
    revision: Revision,
    source: Arc<str>,
    text: String,
    segments: Vec<ProjectionSegment>,
    blocks: Vec<ProjectedBlock>,
    styles: Vec<DisplayStyleSpan>,
    style_max_ends: Vec<usize>,
    source_boundaries: Vec<usize>,
    display_boundaries: Vec<usize>,
}

impl Projection {
    pub fn document_id(&self) -> DocumentId {
        self.document_id
    }
    pub fn revision(&self) -> Revision {
        self.revision
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn blocks(&self) -> &[ProjectedBlock] {
        &self.blocks
    }
    pub fn segments(&self) -> &[ProjectionSegment] {
        &self.segments
    }
    pub fn styles(&self) -> &[DisplayStyleSpan] {
        &self.styles
    }
    /// Nested styles overlapping a half-open visible byte range. A retained
    /// prefix-max-end index avoids scanning all preceding document styles.
    pub fn styles_intersecting(
        &self,
        range: Range<DisplayOffset>,
    ) -> impl Iterator<Item = &DisplayStyleSpan> {
        let first = self
            .style_max_ends
            .partition_point(|end| *end <= range.start.0);
        self.styles[first..]
            .iter()
            .take_while(move |style| style.display_range.start < range.end)
            .filter(move |style| range.start < range.end && range.start < style.display_range.end)
    }

    pub(crate) fn build(document: &ParsedDocument, options: ProjectionOptions) -> Self {
        let mut result = Self {
            document_id: document.document_id,
            revision: document.revision,
            source: document.source.clone(),
            text: String::new(),
            segments: Vec::new(),
            blocks: Vec::new(),
            styles: Vec::new(),
            style_max_ends: Vec::new(),
            source_boundaries: boundaries(&document.source),
            display_boundaries: Vec::new(),
        };
        let mut atom_index = 0;
        for block in document.blocks.iter() {
            while atom_index < document.atoms.len()
                && document.atoms[atom_index].range.end <= block.source_range.start.0
            {
                atom_index += 1;
            }
            let end = document.atoms[atom_index..]
                .partition_point(|atom| atom.range.start < block.source_range.end.0)
                + atom_index;
            let mut folded =
                block_pieces(&document.source, block, &document.atoms[atom_index..end]);
            if document.dialect != crate::MarkdownDialect::CommonMark
                && matches!(block.kind, BlockKind::Heading(_))
                && atom_index == end
            {
                let range = block.source_range.start.0..block.source_range.end.0;
                let text = &document.source[range.clone()];
                let ending = if text.ends_with("\r\n") {
                    2
                } else {
                    usize::from(text.ends_with(['\n', '\r']))
                };
                let at = range.end - ending;
                folded = vec![Piece {
                    range: range.start..at,
                    text: String::new(),
                    kind: SegmentKind::Hidden,
                }];
                if ending > 0 {
                    folded.push(Piece {
                        range: at..range.end,
                        text: "\n".into(),
                        kind: SegmentKind::Transform,
                    });
                }
            }
            let revealed = options.reveal.as_ref().is_some_and(|range| {
                intersects_block(range, &block.source_range, document.source.len())
            });
            let display_start = result.text.len();
            for piece in folded {
                let displayed = if revealed {
                    &document.source[piece.range.clone()]
                } else {
                    &piece.text
                };
                let start = result.text.len();
                result.text.push_str(displayed);
                let stop = result.text.len();
                let kind = if revealed {
                    SegmentKind::Copy
                } else {
                    piece.kind
                };
                if revealed && piece.kind == SegmentKind::Hidden && start != stop {
                    result.styles.push(DisplayStyleSpan {
                        display_range: DisplayOffset(start)..DisplayOffset(stop),
                        kind: StyleKind::Syntax,
                    });
                }
                result.push_segment(ProjectionSegment {
                    source_range: ByteOffset(piece.range.start)..ByteOffset(piece.range.end),
                    display_range: DisplayOffset(start)..DisplayOffset(stop),
                    kind,
                });
            }
            result.blocks.push(ProjectedBlock {
                id: block.id,
                generation: 0,
                source_range: block.source_range.clone(),
                display_range: DisplayOffset(display_start)..DisplayOffset(result.text.len()),
                kind: block.kind.clone(),
                context: block.context.clone(),
                revealed,
            });
        }
        result.display_boundaries = boundaries(&result.text);
        for span in document.styles.iter() {
            let start = result
                .source_to_display(span.source_range.start, Affinity::After)
                .expect("parser ranges are validated");
            let end = result
                .source_to_display(span.source_range.end, Affinity::Before)
                .expect("parser ranges are validated");
            if start < end {
                result.styles.push(DisplayStyleSpan {
                    display_range: start..end,
                    kind: span.kind.clone(),
                });
            }
        }
        result.styles.sort_by_key(|style| {
            (
                style.display_range.start,
                std::cmp::Reverse(style.display_range.end),
            )
        });
        let mut maximum = 0;
        result.style_max_ends = result
            .styles
            .iter()
            .map(|style| {
                maximum = maximum.max(style.display_range.end.0);
                maximum
            })
            .collect();
        for block in &mut result.blocks {
            let mut hash = DefaultHasher::new();
            result.text[block.display_range.start.0..block.display_range.end.0].hash(&mut hash);
            block.kind.hash(&mut hash);
            block.context.hash(&mut hash);
            block.revealed.hash(&mut hash);
            let first_style = result
                .style_max_ends
                .partition_point(|end| *end <= block.display_range.start.0);
            for style in result.styles[first_style..]
                .iter()
                .take_while(|style| style.display_range.start < block.display_range.end)
            {
                if style.display_range.start < block.display_range.end
                    && block.display_range.start < style.display_range.end
                {
                    style
                        .display_range
                        .start
                        .0
                        .saturating_sub(block.display_range.start.0)
                        .hash(&mut hash);
                    style
                        .display_range
                        .end
                        .0
                        .saturating_sub(block.display_range.start.0)
                        .hash(&mut hash);
                    style.kind.hash(&mut hash);
                }
            }
            block.generation = hash.finish();
        }
        result
    }

    fn push_segment(&mut self, segment: ProjectionSegment) {
        if let Some(last) = self.segments.last_mut()
            && last.kind == segment.kind
            && last.source_range.end == segment.source_range.start
            && last.display_range.end == segment.display_range.start
            && segment.kind != SegmentKind::Transform
        {
            last.source_range.end = segment.source_range.end;
            last.display_range.end = segment.display_range.end;
            return;
        }
        self.segments.push(segment);
    }

    pub fn source_to_display(
        &self,
        offset: ByteOffset,
        affinity: Affinity,
    ) -> Result<DisplayOffset, ProjectionError> {
        if offset.0 > self.source.len() || !self.source.is_char_boundary(offset.0) {
            return Err(ProjectionError::InvalidSourceOffset(offset));
        }
        let start = self
            .segments
            .partition_point(|segment| segment.source_range.end < offset);
        let mut candidates = Vec::new();
        for segment in &self.segments[start..] {
            if segment.source_range.start > offset {
                break;
            }
            let value = match segment.kind {
                SegmentKind::Copy => {
                    segment.display_range.start.0 + offset.0 - segment.source_range.start.0
                }
                SegmentKind::Hidden => segment.display_range.start.0,
                SegmentKind::Transform if offset == segment.source_range.start => {
                    segment.display_range.start.0
                }
                SegmentKind::Transform if offset == segment.source_range.end => {
                    segment.display_range.end.0
                }
                SegmentKind::Transform => match affinity {
                    Affinity::Before => segment.display_range.start.0,
                    Affinity::After => segment.display_range.end.0,
                },
            };
            candidates.push(value);
        }
        let selected = choose(candidates, affinity).unwrap_or(0);
        Ok(DisplayOffset(snap(
            &self.display_boundaries,
            selected,
            affinity,
        )))
    }

    pub fn display_to_source(
        &self,
        offset: DisplayOffset,
        affinity: Affinity,
    ) -> Result<ByteOffset, ProjectionError> {
        self.validate_display(offset)?;
        let start = self
            .segments
            .partition_point(|segment| segment.display_range.end < offset);
        let mut candidates = Vec::new();
        for segment in &self.segments[start..] {
            if segment.display_range.start > offset {
                break;
            }
            match segment.kind {
                SegmentKind::Copy => candidates
                    .push(segment.source_range.start.0 + offset.0 - segment.display_range.start.0),
                SegmentKind::Hidden => {
                    candidates.push(segment.source_range.start.0);
                    candidates.push(segment.source_range.end.0);
                }
                SegmentKind::Transform if offset == segment.display_range.start => {
                    candidates.push(segment.source_range.start.0)
                }
                SegmentKind::Transform if offset == segment.display_range.end => {
                    candidates.push(segment.source_range.end.0)
                }
                SegmentKind::Transform => {
                    candidates.push(segment.source_range.start.0);
                    candidates.push(segment.source_range.end.0);
                }
            }
        }
        Ok(ByteOffset(snap(
            &self.source_boundaries,
            choose(candidates, affinity).unwrap_or(0),
            affinity,
        )))
    }

    fn validate_display(&self, offset: DisplayOffset) -> Result<(), ProjectionError> {
        if offset.0 > self.text.len() || !self.text.is_char_boundary(offset.0) {
            return Err(ProjectionError::InvalidDisplayOffset(offset));
        }
        if self.display_boundaries.binary_search(&offset.0).is_err() {
            return Err(ProjectionError::NotGraphemeBoundary(offset));
        }
        Ok(())
    }

    /// Convert one user edit to an atomic source transaction. Transforms can be
    /// replaced whole, but partial transformed spelling requires source reveal.
    /// Source and history are untouched until the host applies the transaction.
    pub fn projected_edit(
        &self,
        snapshot: &Snapshot,
        range: Range<DisplayOffset>,
        inserted: &str,
        affinity: Affinity,
        origin: TransactionOrigin,
    ) -> Result<Transaction, ProjectionError> {
        if snapshot.document_id() != self.document_id || snapshot.revision() != self.revision {
            return Err(ProjectionError::StaleSnapshot);
        }
        if range.start > range.end {
            return Err(ProjectionError::ReversedRange);
        }
        self.validate_display(range.start)?;
        self.validate_display(range.end)?;
        for segment in &self.segments {
            if segment.kind != SegmentKind::Transform {
                continue;
            }
            let a = segment.display_range.start;
            let b = segment.display_range.end;
            if (a < range.start && range.start < b) || (a < range.end && range.end < b) {
                return Err(ProjectionError::RevealRequired {
                    source_range: segment.source_range.clone(),
                });
            }
        }
        let source = if range.start == range.end {
            let at = self.display_to_source(range.start, affinity)?;
            at..at
        } else {
            self.display_to_source(range.start, Affinity::After)?
                ..self.display_to_source(range.end, Affinity::Before)?
        };
        if source.start > source.end {
            return Err(ProjectionError::ReversedRange);
        }
        // Hiding a delimiter can change grapheme boundaries (e.g. a combining
        // mark after a backtick). Snapping must not silently move an intended
        // edit past visible text or delete a different grapheme/line ending.
        let start_roundtrip = self.source_to_display(
            source.start,
            if range.start == range.end {
                affinity
            } else {
                Affinity::After
            },
        )?;
        let end_roundtrip = self.source_to_display(
            source.end,
            if range.start == range.end {
                affinity
            } else {
                Affinity::Before
            },
        )?;
        if start_roundtrip != range.start || end_roundtrip != range.end {
            let relevant: Vec<_> = self
                .blocks
                .iter()
                .filter(|block| {
                    block.display_range.start <= range.end && range.start <= block.display_range.end
                })
                .collect();
            let start = relevant
                .first()
                .map_or(source.start, |block| block.source_range.start);
            let end = relevant
                .last()
                .map_or(source.end, |block| block.source_range.end);
            return Err(ProjectionError::RevealRequired {
                source_range: start..end,
            });
        }
        Ok(Transaction::new(snapshot)
            .replace(source.clone(), inserted)
            .with_selection(Selection {
                anchor: ByteOffset(source.start.0 + inserted.len()),
                head: ByteOffset(source.start.0 + inserted.len()),
                affinity,
            })
            .with_origin(origin)
            .with_grapheme_selection(Affinity::After))
    }
}

fn choose(values: Vec<usize>, affinity: Affinity) -> Option<usize> {
    match affinity {
        Affinity::Before => values.into_iter().min(),
        Affinity::After => values.into_iter().max(),
    }
}
fn boundaries(text: &str) -> Vec<usize> {
    if text.is_ascii() {
        let bytes = text.as_bytes();
        return (0..=text.len())
            .filter(|index| {
                *index == 0 || bytes.get(*index) != Some(&b'\n') || bytes[*index - 1] != b'\r'
            })
            .collect();
    }
    text.grapheme_indices(true)
        .map(|(index, _)| index)
        .chain([text.len()])
        .collect()
}
fn snap(boundaries: &[usize], value: usize, affinity: Affinity) -> usize {
    match boundaries.binary_search(&value) {
        Ok(_) => value,
        Err(index) => match affinity {
            Affinity::Before => boundaries[index.saturating_sub(1)],
            Affinity::After => boundaries[index.min(boundaries.len() - 1)],
        },
    }
}
fn intersects_block(
    reveal: &Range<ByteOffset>,
    block: &Range<ByteOffset>,
    source_len: usize,
) -> bool {
    if reveal.start == reveal.end {
        block.start <= reveal.start
            && (reveal.start < block.end
                || reveal.start.0 == source_len && block.end.0 == source_len)
    } else {
        reveal.start < block.end && block.start < reveal.end
    }
}

#[derive(Clone)]
struct Piece {
    range: Range<usize>,
    text: String,
    kind: SegmentKind,
}

fn table_pieces(source: &str, block: &Block, atoms: &[Atom]) -> Option<Vec<Piece>> {
    let table = glyphfolio_editor_core::MarkdownTable::at(source, block.source_range.start)?;
    if table.range != block.source_range
        || table
            .rows
            .iter()
            .any(|row| row.cells.len() != table.separator.cells.len())
    {
        return None;
    }
    let mut rows: Vec<_> = table.rows.iter().collect();
    rows.push(&table.separator);
    rows.sort_by_key(|row| row.range.start);
    let mut output = Vec::new();
    for row in rows {
        if row.range == table.separator.range {
            output.push(Piece {
                range: row.range.start.0..row.range.end.0,
                text: String::new(),
                kind: SegmentKind::Hidden,
            });
            continue;
        }
        let mut cursor = row.range.start.0;
        for (column, cell) in row.cells.iter().enumerate() {
            output.push(Piece {
                range: cursor..cell.start.0,
                text: if column == 0 {
                    String::new()
                } else {
                    "\t".into()
                },
                kind: if column == 0 {
                    SegmentKind::Hidden
                } else {
                    SegmentKind::Transform
                },
            });
            let leaf = Block {
                id: block.id,
                source_range: cell.clone(),
                kind: BlockKind::Paragraph,
                context: BlockContext::default(),
            };
            let first = atoms.partition_point(|atom| atom.range.end <= cell.start.0);
            let last = first + atoms[first..].partition_point(|atom| atom.range.start < cell.end.0);
            if cell.is_empty() {
                output.push(Piece {
                    range: cell.start.0..cell.end.0,
                    text: String::new(),
                    kind: SegmentKind::Copy,
                });
            } else {
                output.extend(block_pieces(source, &leaf, &atoms[first..last]));
            }
            cursor = cell.end.0;
        }
        output.push(Piece {
            range: cursor..row.range.end.0,
            text: "\n".into(),
            kind: SegmentKind::Transform,
        });
    }
    Some(output)
}

fn block_pieces(source: &str, block: &Block, atoms: &[Atom]) -> Vec<Piece> {
    let range = block.source_range.start.0..block.source_range.end.0;
    if block.kind == BlockKind::Table {
        return table_pieces(source, block, atoms).unwrap_or_else(|| {
            vec![Piece {
                range: range.clone(),
                text: source[range.clone()].into(),
                kind: SegmentKind::Copy,
            }]
        });
    }
    // An empty heading or loaded fence has no content anchor. Showing its
    // source avoids mapping a visual placeholder into a hidden closing marker.
    if matches!(
        block.kind,
        BlockKind::Heading(_) | BlockKind::CodeBlock { .. }
    ) && !atoms.iter().any(|atom| {
        !atom.text.is_empty() && range.start <= atom.range.start && atom.range.end <= range.end
    }) {
        return vec![Piece {
            text: source[range.clone()].into(),
            range,
            kind: SegmentKind::Copy,
        }];
    }
    if block.kind == BlockKind::Blank && source[range.clone()].contains('\r') {
        return source[range.clone()]
            .grapheme_indices(true)
            .map(|(offset, grapheme)| {
                let normalized = grapheme == "\r\n" || grapheme == "\r";
                Piece {
                    range: range.start + offset..range.start + offset + grapheme.len(),
                    text: if normalized {
                        "\n".into()
                    } else {
                        grapheme.into()
                    },
                    kind: if normalized {
                        SegmentKind::Transform
                    } else {
                        SegmentKind::Copy
                    },
                }
            })
            .collect();
    }
    if matches!(
        block.kind,
        BlockKind::Blank | BlockKind::Unsupported | BlockKind::Html | BlockKind::ThematicBreak
    ) {
        return vec![Piece {
            text: source[range.clone()].into(),
            range,
            kind: SegmentKind::Copy,
        }];
    }
    let mut result = Vec::new();
    let mut cursor = range.start;
    for atom in atoms {
        if atom.range.start < cursor || atom.range.end > range.end {
            continue;
        }
        if cursor < atom.range.start {
            result.push(Piece {
                range: cursor..atom.range.start,
                text: String::new(),
                kind: SegmentKind::Hidden,
            });
        }
        let raw = &source[atom.range.clone()];
        let kind = if atom.raw || raw == atom.text {
            SegmentKind::Copy
        } else {
            SegmentKind::Transform
        };
        result.push(Piece {
            range: atom.range.clone(),
            text: if atom.raw {
                raw.into()
            } else {
                atom.text.clone()
            },
            kind,
        });
        cursor = atom.range.end;
    }
    if cursor < range.end {
        let tail = &source[cursor..range.end];
        let terminal = if tail.ends_with("\r\n") {
            2
        } else if tail.ends_with(['\n', '\r']) {
            1
        } else {
            0
        };
        let rendered_ends_line = result
            .last()
            .is_some_and(|piece| piece.text.ends_with('\n'));
        if terminal > 0 && !rendered_ends_line {
            let end = range.end - terminal;
            if cursor < end {
                result.push(Piece {
                    range: cursor..end,
                    text: String::new(),
                    kind: SegmentKind::Hidden,
                });
            }
            result.push(Piece {
                range: end..range.end,
                text: "\n".into(),
                kind: if terminal == 1 && source.as_bytes()[end] == b'\n' {
                    SegmentKind::Copy
                } else {
                    SegmentKind::Transform
                },
            });
        } else {
            result.push(Piece {
                range: cursor..range.end,
                text: String::new(),
                kind: SegmentKind::Hidden,
            });
        }
    }
    if result.is_empty() {
        result.push(Piece {
            range,
            text: String::new(),
            kind: SegmentKind::Copy,
        });
    }
    result
}
