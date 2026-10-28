//! Original, source-preserving Markdown parsing and projection.
//!
//! CommonMark semantics come from the pinned low-level parser. This crate owns
//! semantic block identity, view projections, source/display coordinates and
//! transaction mapping. Derived text is never an independently editable document.
//! Raw HTML, images and definitions remain editable source fallbacks. No UI or
//! existing editor controller is used.

mod assets;
pub mod editing;
pub use assets::{ImageReference, image_references};
mod html;
pub use html::{html_to_markdown, safe_link};
mod parser;
mod projection;

pub use parser::{MarkdownDialect, MarkdownParser, ParseMode, ParseStats, ParsedDocument};
pub use projection::{
    DisplayOffset, DisplayStyleSpan, ProjectedBlock, Projection, ProjectionError,
    ProjectionOptions, ProjectionSegment, SegmentKind,
};

use glyphfolio_editor_core::ByteOffset;
use std::ops::Range;

/// Stable within a parser lifetime, never recycled for a different document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum BlockKind {
    Paragraph,
    Heading(u8),
    CodeBlock {
        language: Option<String>,
        fenced: bool,
    },
    Table,
    ThematicBreak,
    Html,
    Blank,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ListKind {
    Unordered,
    Ordered { start: u64 },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct BlockContext {
    pub quote_depth: usize,
    pub list_depth: usize,
    pub list_kind: Option<ListKind>,
    pub item_number: Option<u64>,
    /// Only the first semantic leaf on an item's marker line gets an adornment.
    pub list_item_start: bool,
}

/// Leaf blocks partition every source byte, including blank/unsupported source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub id: BlockId,
    pub source_range: Range<ByteOffset>,
    pub kind: BlockKind,
    pub context: BlockContext,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum StyleKind {
    Emphasis,
    Strong,
    Strikethrough,
    TaskCheckbox {
        checked: bool,
    },
    Image {
        destination: String,
    },
    Code,
    Link {
        destination: String,
        title: String,
    },
    /// Revealed syntax can be dimmed without changing source or text geometry.
    Syntax,
}

/// Nested source styles are independent from the ordered projection segments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyleSpan {
    pub source_range: Range<ByteOffset>,
    pub kind: StyleKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Atom {
    pub range: Range<usize>,
    pub text: String,
    pub raw: bool,
}

pub(crate) fn bytes(range: Range<usize>) -> Range<ByteOffset> {
    ByteOffset(range.start)..ByteOffset(range.end)
}
