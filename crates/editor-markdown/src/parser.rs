use crate::projection::{Projection, ProjectionOptions};
use crate::{Atom, Block, BlockContext, BlockId, BlockKind, ListKind, StyleKind, StyleSpan, bytes};
use glyphfolio_editor_core::{
    Affinity, ByteOffset, CommittedChange, DocumentId, Revision, Snapshot,
};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::borrow::Cow;
use std::{ops::Range, sync::Arc};
use unicode_segmentation::UnicodeSegmentation;

/// Explicit grammar policy. CommonMark stays the reference/test default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MarkdownDialect {
    #[default]
    CommonMark,
    Gfm,
    GfmWiki,
}
impl MarkdownDialect {
    fn options(self) -> Options {
        match self {
            Self::CommonMark => Options::empty(),
            Self::Gfm | Self::GfmWiki => {
                Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_TABLES
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseMode {
    Full,
    IncrementalParagraph,
    Unchanged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseStats {
    pub mode: ParseMode,
    /// Source bytes in parsed regions, excluding full-source copy/range shifts.
    /// CRLF normalization can shorten the actual parser input within a region.
    pub parsed_bytes: usize,
    pub reused_blocks: usize,
    pub fallback_reason: Option<&'static str>,
}

#[derive(Clone, Debug)]
pub struct ParsedDocument {
    pub(crate) dialect: MarkdownDialect,
    pub(crate) document_id: DocumentId,
    pub(crate) revision: Revision,
    pub(crate) source: Arc<str>,
    pub(crate) blocks: Arc<[Block]>,
    pub(crate) styles: Arc<[StyleSpan]>,
    pub(crate) atoms: Arc<[Atom]>,
    pub(crate) stats: ParseStats,
}

impl ParsedDocument {
    pub fn document_id(&self) -> DocumentId {
        self.document_id
    }
    pub fn revision(&self) -> Revision {
        self.revision
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }
    pub fn styles(&self) -> &[StyleSpan] {
        &self.styles
    }
    pub fn stats(&self) -> &ParseStats {
        &self.stats
    }
    pub fn project(&self, options: ProjectionOptions) -> Projection {
        Projection::build(self, options)
    }
    /// Reference HTML is for grammar qualification/export, never document storage.
    pub fn reference_html(&self) -> String {
        let mut html = String::new();
        let input = CommonMarkInput::new(&self.source);
        pulldown_cmark::html::push_html(
            &mut html,
            Parser::new_ext(&input.text, self.dialect.options()),
        );
        html
    }
}

#[derive(Default)]
pub struct MarkdownParser {
    dialect: MarkdownDialect,
    previous: Option<ParsedDocument>,
    next_block: u64,
}

impl MarkdownParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_dialect(dialect: MarkdownDialect) -> Self {
        Self {
            dialect,
            ..Self::default()
        }
    }

    /// Full reference parsing with prefix/suffix identity reconciliation. Use
    /// [`Self::parse_change`] when exact edit anchors are available, particularly
    /// for insertion/deletion among identical blocks.
    pub fn parse(&mut self, snapshot: &Snapshot) -> ParsedDocument {
        if let Some(old) = &self.previous
            && old.document_id == snapshot.document_id()
            && old.revision == snapshot.revision()
        {
            let mut result = old.clone();
            result.stats = ParseStats {
                mode: ParseMode::Unchanged,
                parsed_bytes: 0,
                reused_blocks: old.blocks.len(),
                fallback_reason: None,
            };
            return result;
        }
        self.full(snapshot, None, None)
    }

    /// Conservative fast path for an ASCII-word edit inside an unstyled,
    /// top-level plain paragraph. All structural/reference/inline changes fall
    /// back to the full reference parser. Source materialization and suffix
    /// range shifting remain O(N); this is not a dirty-line complexity promise.
    pub fn parse_change(&mut self, change: &CommittedChange) -> ParsedDocument {
        if let Some(result) = self.incremental(change) {
            self.previous = Some(result.clone());
            return result;
        }
        self.full(
            &change.snapshot,
            Some("edit is not a proven local plain-paragraph change"),
            Some(change),
        )
    }

    fn full(
        &mut self,
        snapshot: &Snapshot,
        reason: Option<&'static str>,
        change: Option<&CommittedChange>,
    ) -> ParsedDocument {
        let source: Arc<str> = snapshot.text().into();
        let (drafts, styles, atoms) = parse_raw(&source, self.dialect);
        let mut blocks: Vec<_> = drafts
            .into_iter()
            .map(|draft| Block {
                id: BlockId(0),
                source_range: bytes(draft.range),
                kind: draft.kind,
                context: draft.context,
            })
            .collect();
        let mut reused = 0;
        if let Some(old) = &self.previous
            && old.document_id == snapshot.document_id()
        {
            let anchored = change.filter(|change| {
                change.document_id == old.document_id
                    && change.snapshot.document_id() == old.document_id
                    && change.before_revision == old.revision
                    && change.revision == snapshot.revision()
                    && change.revision.0 > old.revision.0
            });
            if let Some(change) = anchored {
                // Unchanged anchors win first, so inserting an identical block
                // before one does not transfer the old block's identity to it.
                let mut used = std::collections::HashSet::new();
                let indices: std::collections::HashMap<_, _> = blocks
                    .iter()
                    .enumerate()
                    .map(|(index, block)| {
                        (
                            (block.source_range.start.0, block.source_range.end.0),
                            index,
                        )
                    })
                    .collect();
                for unchanged in [true, false] {
                    for previous in old.blocks.iter() {
                        if used.contains(&previous.id) {
                            continue;
                        }
                        let start_affinity = if unchanged {
                            Affinity::After
                        } else {
                            Affinity::Before
                        };
                        let end_affinity = if unchanged {
                            Affinity::Before
                        } else {
                            Affinity::After
                        };
                        let Ok(start) = change
                            .change_map
                            .map_offset(previous.source_range.start, start_affinity)
                        else {
                            continue;
                        };
                        let Ok(end) = change
                            .change_map
                            .map_offset(previous.source_range.end, end_affinity)
                        else {
                            continue;
                        };
                        if let Some(index) = indices.get(&(start.0, end.0)) {
                            let new = &mut blocks[*index];
                            if new.id == BlockId(0)
                                && new.kind == previous.kind
                                && new.context == previous.context
                                && (!unchanged
                                    || old.source.get(
                                        previous.source_range.start.0..previous.source_range.end.0,
                                    ) == source.get(start.0..end.0))
                            {
                                new.id = previous.id;
                                used.insert(previous.id);
                                reused += 1;
                            }
                        }
                    }
                }
            } else {
                let equivalent = |a: &Block, b: &Block| {
                    a.kind == b.kind
                        && a.context == b.context
                        && old.source.get(a.source_range.start.0..a.source_range.end.0)
                            == source.get(b.source_range.start.0..b.source_range.end.0)
                };
                let prefix = old
                    .blocks
                    .iter()
                    .zip(&blocks)
                    .take_while(|(a, b)| equivalent(a, b))
                    .count();
                for (new, previous) in blocks.iter_mut().zip(old.blocks.iter()).take(prefix) {
                    new.id = previous.id;
                    reused += 1;
                }
                let suffix = old.blocks[prefix..]
                    .iter()
                    .rev()
                    .zip(blocks[prefix..].iter().rev())
                    .take_while(|(a, b)| equivalent(a, b))
                    .count();
                for (new, previous) in blocks
                    .iter_mut()
                    .rev()
                    .zip(old.blocks.iter().rev())
                    .take(suffix)
                {
                    new.id = previous.id;
                    reused += 1;
                }
                if old.blocks.len() == prefix + suffix + 1 && blocks.len() == prefix + suffix + 1 {
                    let previous = &old.blocks[prefix];
                    let new = &mut blocks[prefix];
                    let a = &old.source[previous.source_range.start.0..previous.source_range.end.0];
                    let b = &source[new.source_range.start.0..new.source_range.end.0];
                    let shared = a.chars().zip(b.chars()).take_while(|(a, b)| a == b).count();
                    if previous.kind == new.kind && previous.context == new.context && shared > 0 {
                        new.id = previous.id;
                        reused += 1;
                    }
                }
            }
        }
        for block in &mut blocks {
            if block.id == BlockId(0) {
                self.next_block = self
                    .next_block
                    .checked_add(1)
                    .expect("block identity space exhausted");
                block.id = BlockId(self.next_block);
            }
        }
        let result = ParsedDocument {
            dialect: self.dialect,
            document_id: snapshot.document_id(),
            revision: snapshot.revision(),
            stats: ParseStats {
                mode: ParseMode::Full,
                parsed_bytes: source.len(),
                reused_blocks: reused,
                fallback_reason: reason,
            },
            source,
            blocks: blocks.into(),
            styles: styles.into(),
            atoms: atoms.into(),
        };
        self.previous = Some(result.clone());
        result
    }

    fn incremental(&self, change: &CommittedChange) -> Option<ParsedDocument> {
        let old = self.previous.as_ref()?;
        if old.document_id != change.document_id
            || old.document_id != change.snapshot.document_id()
            || old.revision != change.before_revision
            || change.revision.0 <= old.revision.0
            || change.changes.len() != 1
            || change.snapshot.revision() != change.revision
        {
            return None;
        }
        let edit = &change.changes[0];
        let start = edit.range.start.0;
        let end = edit.range.end.0;
        if !edit
            .inserted
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric())
        {
            return None;
        }
        let deleted = old.source.get(start..end)?;
        if !deleted.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
            return None;
        }
        let index = old.blocks.iter().position(|block| {
            block.kind == BlockKind::Paragraph
                && block.context == BlockContext::default()
                && block.source_range.start.0 <= start
                && end <= block.source_range.end.0
        })?;
        let target = &old.blocks[index];
        let original = &old.source[target.source_range.start.0..target.source_range.end.0];
        let body = original.strip_suffix('\n').unwrap_or(original);
        if body.is_empty()
            || !body.as_bytes()[0].is_ascii_alphanumeric()
            || !body
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b' ')
            || end > target.source_range.start.0 + body.len()
        {
            return None;
        }
        let source: Arc<str> = change.snapshot.text().into();
        let delta = edit.inserted.len() as isize - (end - start) as isize;
        let target_end = target.source_range.end.0.checked_add_signed(delta)?;
        let replacement = source.get(target.source_range.start.0..target_end)?;
        let body = replacement.strip_suffix('\n').unwrap_or(replacement);
        if body.is_empty() || !body.as_bytes()[0].is_ascii_alphanumeric() {
            return None;
        }
        // Verify actual snapshot bytes agree with the declared original edit.
        if old.source.get(..start)? != source.get(..start)?
            || old.source.get(end..)? != source.get(start + edit.inserted.len()..)?
        {
            return None;
        }
        let (local, local_styles, mut local_atoms) = parse_raw(replacement, self.dialect);
        if local.len() != 1 || local[0].kind != BlockKind::Paragraph || !local_styles.is_empty() {
            return None;
        }
        let mut blocks = old.blocks.to_vec();
        blocks[index].source_range.end = ByteOffset(target_end);
        for block in &mut blocks[index + 1..] {
            block.source_range = shift_bytes(&block.source_range, delta)?;
        }
        let mut styles = old.styles.to_vec();
        for style in &mut styles {
            if style.source_range.start.0 >= target.source_range.end.0 {
                style.source_range = shift_bytes(&style.source_range, delta)?;
            } else if style.source_range.end.0 > target.source_range.start.0 {
                return None;
            }
        }
        let mut atoms: Vec<_> = old
            .atoms
            .iter()
            .filter(|atom| atom.range.end <= target.source_range.start.0)
            .cloned()
            .collect();
        for atom in &mut local_atoms {
            atom.range.start += target.source_range.start.0;
            atom.range.end += target.source_range.start.0;
        }
        atoms.extend(local_atoms);
        for atom in old
            .atoms
            .iter()
            .filter(|atom| atom.range.start >= target.source_range.end.0)
        {
            let mut atom = atom.clone();
            atom.range = atom.range.start.checked_add_signed(delta)?
                ..atom.range.end.checked_add_signed(delta)?;
            atoms.push(atom);
        }
        Some(ParsedDocument {
            dialect: self.dialect,
            document_id: old.document_id,
            revision: change.revision,
            stats: ParseStats {
                mode: ParseMode::IncrementalParagraph,
                parsed_bytes: replacement.len(),
                reused_blocks: blocks.len(),
                fallback_reason: None,
            },
            source,
            blocks: blocks.into(),
            styles: styles.into(),
            atoms: atoms.into(),
        })
    }
}

fn shift_bytes(range: &Range<ByteOffset>, delta: isize) -> Option<Range<ByteOffset>> {
    Some(
        ByteOffset(range.start.0.checked_add_signed(delta)?)
            ..ByteOffset(range.end.0.checked_add_signed(delta)?),
    )
}

#[derive(Clone)]
struct Draft {
    range: Range<usize>,
    kind: BlockKind,
    context: BlockContext,
}

#[derive(Clone)]
struct ListFrame {
    kind: ListKind,
    next: Option<u64>,
}

fn context(quotes: usize, lists: &[ListFrame], items: &[Option<u64>]) -> BlockContext {
    BlockContext {
        quote_depth: quotes,
        list_depth: lists.len(),
        list_kind: lists.last().map(|list| list.kind),
        item_number: items.last().copied().flatten(),
        list_item_start: false,
    }
}

/// Event offsets are not a lossless token stream. We retain every source gap,
/// explicitly partition container-prefixed lines, and keep unsupported constructs
/// as source. Grammar decisions remain those of the full CommonMark parser.
fn parse_raw(source: &str, dialect: MarkdownDialect) -> (Vec<Draft>, Vec<StyleSpan>, Vec<Atom>) {
    let mut drafts = Vec::new();
    let mut styles = Vec::new();
    let mut atoms = Vec::new();
    let mut quotes = 0;
    let mut lists: Vec<ListFrame> = Vec::new();
    let mut items = Vec::new();
    let mut item_drafts = Vec::new();
    let mut item_starts = std::collections::HashSet::new();
    let mut explicit_depth = 0usize;
    let mut tight: Option<Draft> = None;
    let mut images = 0usize;
    let mut image_alts: Vec<(usize, String)> = Vec::new();
    let flush = |draft: &mut Option<Draft>, output: &mut Vec<Draft>| {
        if let Some(draft) = draft.take() {
            output.push(draft);
        }
    };
    let input = CommonMarkInput::new(source);
    for (event, normalized_range) in
        Parser::new_ext(&input.text, dialect.options()).into_offset_iter()
    {
        let range = input.source_range(normalized_range);
        match event {
            Event::Start(tag) => {
                let inline_container = matches!(
                    &tag,
                    Tag::Emphasis
                        | Tag::Strong
                        | Tag::Strikethrough
                        | Tag::Link { .. }
                        | Tag::Image { .. }
                );
                let kind = match &tag {
                    Tag::Paragraph => Some(BlockKind::Paragraph),
                    Tag::Heading { level, .. } => Some(BlockKind::Heading(*level as u8)),
                    Tag::CodeBlock(kind) => Some(match kind {
                        CodeBlockKind::Fenced(info) => BlockKind::CodeBlock {
                            language: info.split_whitespace().next().map(str::to_owned),
                            fenced: true,
                        },
                        CodeBlockKind::Indented => BlockKind::CodeBlock {
                            language: None,
                            fenced: false,
                        },
                    }),
                    Tag::HtmlBlock => Some(BlockKind::Html),
                    Tag::Table(_) => Some(BlockKind::Table),
                    _ => None,
                };
                if let Some(kind) = kind {
                    flush(&mut tight, &mut drafts);
                    drafts.push(Draft {
                        range: range.clone(),
                        kind,
                        context: context(quotes, &lists, &items),
                    });
                    explicit_depth += 1;
                }
                match tag {
                    Tag::BlockQuote(_) => {
                        flush(&mut tight, &mut drafts);
                        quotes += 1;
                    }
                    Tag::List(start) => {
                        flush(&mut tight, &mut drafts);
                        lists.push(ListFrame {
                            kind: start
                                .map_or(ListKind::Unordered, |start| ListKind::Ordered { start }),
                            next: start,
                        });
                    }
                    Tag::Item => {
                        flush(&mut tight, &mut drafts);
                        let ordinal = lists.last_mut().and_then(|list| {
                            let next = list.next;
                            list.next = next.and_then(|value| value.checked_add(1));
                            next
                        });
                        items.push(ordinal);
                        item_starts.insert((line_start(source, range.start), lists.len()));
                        item_drafts.push((range.clone(), drafts.len()));
                    }
                    Tag::Emphasis => styles.push(StyleSpan {
                        source_range: bytes(range.clone()),
                        kind: StyleKind::Emphasis,
                    }),
                    Tag::Strikethrough => styles.push(StyleSpan {
                        source_range: bytes(range.clone()),
                        kind: StyleKind::Strikethrough,
                    }),
                    Tag::Strong => styles.push(StyleSpan {
                        source_range: bytes(range.clone()),
                        kind: StyleKind::Strong,
                    }),
                    Tag::Link {
                        dest_url, title, ..
                    } => styles.push(StyleSpan {
                        source_range: bytes(range.clone()),
                        kind: StyleKind::Link {
                            destination: dest_url.into_string(),
                            title: title.into_string(),
                        },
                    }),
                    Tag::Image { dest_url, .. } => {
                        styles.push(StyleSpan {
                            source_range: bytes(range.clone()),
                            kind: StyleKind::Image {
                                destination: dest_url.into_string(),
                            },
                        });
                        image_alts.push((atoms.len(), String::new()));
                        atoms.push(Atom {
                            range: range.clone(),
                            text: source[range.clone()].into(),
                            raw: dialect == MarkdownDialect::CommonMark,
                        });
                        images += 1;
                    }
                    _ => {}
                }
                if explicit_depth == 0 && !items.is_empty() && inline_container {
                    extend_tight(&mut tight, &range, context(quotes, &lists, &items));
                }
            }
            Event::End(tag) => match tag {
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::CodeBlock
                | TagEnd::HtmlBlock
                | TagEnd::Table => {
                    explicit_depth = explicit_depth.saturating_sub(1);
                }
                TagEnd::BlockQuote(_) => {
                    flush(&mut tight, &mut drafts);
                    quotes = quotes.saturating_sub(1);
                }
                TagEnd::List(_) => {
                    flush(&mut tight, &mut drafts);
                    lists.pop();
                }
                TagEnd::Item => {
                    flush(&mut tight, &mut drafts);
                    if let Some((range, count)) = item_drafts.pop()
                        && drafts.len() == count
                    {
                        drafts.push(Draft {
                            range,
                            kind: BlockKind::Paragraph,
                            context: context(quotes, &lists, &items),
                        });
                    }
                    items.pop();
                }
                TagEnd::Image => {
                    images = images.saturating_sub(1);
                    if let Some((index, alt)) = image_alts.pop()
                        && dialect != MarkdownDialect::CommonMark
                    {
                        atoms[index].text = if alt.is_empty() { "Image".into() } else { alt };
                    }
                }
                _ => {}
            },
            Event::Text(text) => {
                if let Some((_, alt)) = image_alts.last_mut() {
                    alt.push_str(&text);
                }
                if explicit_depth == 0 && !items.is_empty() {
                    extend_tight(&mut tight, &range, context(quotes, &lists, &items));
                }
                if images == 0 {
                    text_atoms(source, range, &text, &mut atoms);
                }
            }
            Event::Code(text) => {
                if let Some((_, alt)) = image_alts.last_mut() {
                    alt.push_str(&text);
                }
                if explicit_depth == 0 && !items.is_empty() {
                    extend_tight(&mut tight, &range, context(quotes, &lists, &items));
                }
                styles.push(StyleSpan {
                    source_range: bytes(range.clone()),
                    kind: StyleKind::Code,
                });
                if images == 0 {
                    code_atoms(source, range, &text, &mut atoms);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if explicit_depth == 0 && !items.is_empty() {
                    extend_tight(&mut tight, &range, context(quotes, &lists, &items));
                }
                if images == 0 {
                    atoms.push(Atom {
                        range,
                        text: if matches!(event, Event::SoftBreak) {
                            " "
                        } else {
                            "\n"
                        }
                        .into(),
                        raw: false,
                    });
                }
            }
            Event::Html(text) | Event::InlineHtml(text) => {
                if explicit_depth == 0 && !items.is_empty() {
                    extend_tight(&mut tight, &range, context(quotes, &lists, &items));
                }
                if images == 0 {
                    atoms.push(Atom {
                        range,
                        text: if dialect != MarkdownDialect::CommonMark
                            && matches!(
                                text.trim().to_ascii_lowercase().as_str(),
                                "<br>" | "<br/>" | "<br />"
                            ) {
                            "\n".into()
                        } else {
                            text.clone().into_string()
                        },
                        raw: !(dialect != MarkdownDialect::CommonMark
                            && matches!(
                                text.trim().to_ascii_lowercase().as_str(),
                                "<br>" | "<br/>" | "<br />"
                            )),
                    });
                }
            }
            Event::TaskListMarker(checked) => {
                if explicit_depth == 0 && !items.is_empty() {
                    extend_tight(&mut tight, &range, context(quotes, &lists, &items));
                }
                styles.push(StyleSpan {
                    source_range: bytes(range.clone()),
                    kind: StyleKind::TaskCheckbox { checked },
                });
                atoms.push(Atom {
                    range,
                    text: if checked { "☑ " } else { "☐ " }.into(),
                    raw: false,
                });
            }
            Event::Rule => {
                flush(&mut tight, &mut drafts);
                drafts.push(Draft {
                    range,
                    kind: BlockKind::ThematicBreak,
                    context: context(quotes, &lists, &items),
                });
            }
            _ => {
                if images == 0 {
                    atoms.push(Atom {
                        range: range.clone(),
                        text: source[range].into(),
                        raw: true,
                    });
                }
            }
        }
    }
    flush(&mut tight, &mut drafts);
    for draft in &mut drafts {
        draft.range.start = line_start(source, draft.range.start);
        draft.range.end = line_end(source, draft.range.end);
    }
    drafts.sort_by_key(|draft| (draft.range.start, draft.range.end));
    let mut partition = Vec::new();
    let mut cursor = 0;
    for mut draft in drafts {
        if draft.range.end <= cursor {
            continue;
        }
        if draft.range.start < cursor {
            draft.range.start = cursor;
        }
        if draft.range.start > cursor {
            partition.push(fallback(source, cursor..draft.range.start));
        }
        cursor = draft.range.end;
        partition.push(draft);
    }
    if cursor < source.len() {
        partition.push(fallback(source, cursor..source.len()));
    }
    if partition.is_empty() {
        partition.push(fallback(source, 0..source.len()));
    }
    for block in &mut partition {
        block.context.list_item_start =
            item_starts.contains(&(block.range.start, block.context.list_depth));
    }
    if dialect == MarkdownDialect::GfmWiki {
        wiki_atoms(source, &mut atoms, &mut styles);
    }
    atoms.sort_by(|a, b| {
        a.range
            .start
            .cmp(&b.range.start)
            .then_with(|| b.range.end.cmp(&a.range.end))
    });
    (partition, styles, atoms)
}

fn wiki_atoms(source: &str, atoms: &mut Vec<Atom>, styles: &mut Vec<StyleSpan>) {
    let mut cursor = 0;
    while let Some(relative) = source[cursor..].find("[[") {
        let start = cursor + relative;
        cursor = start + 2;
        if source[..start]
            .bytes()
            .rev()
            .take_while(|b| *b == b'\\')
            .count()
            % 2
            == 1
        {
            continue;
        }
        let Some(close) = source[cursor..].find("]]") else {
            break;
        };
        let end = cursor + close + 2;
        let inner = &source[cursor..end - 2];
        if inner.contains(['\n', '\r', '[', ']']) || inner.trim().is_empty() {
            continue;
        }
        if styles
            .iter()
            .any(|span| span.source_range.start.0 < end && start < span.source_range.end.0)
        {
            continue;
        }
        let mut coverage: Vec<_> = atoms
            .iter()
            .filter(|atom| atom.range.start < end && start < atom.range.end)
            .collect();
        coverage.sort_by_key(|atom| atom.range.start);
        let mut covered = start;
        let valid = coverage.iter().all(|atom| {
            if atom.raw || atom.text != source[atom.range.clone()] || atom.range.start > covered {
                return false;
            }
            covered = covered.max(atom.range.end);
            true
        }) && covered >= end;
        if !valid {
            continue;
        }
        let (target, label_start) = if let Some(pipe) = inner.find('|') {
            (&inner[..pipe], cursor + pipe + 1)
        } else {
            (inner, cursor)
        };
        if target.trim().is_empty() || label_start == end - 2 {
            continue;
        }
        let mut replacement = Vec::new();
        for atom in atoms.drain(..) {
            if atom.range.start >= end || atom.range.end <= start {
                replacement.push(atom);
                continue;
            }
            if atom.range.start < start {
                replacement.push(Atom {
                    range: atom.range.start..start,
                    text: source[atom.range.start..start].into(),
                    raw: false,
                });
            }
            if atom.range.end > end {
                replacement.push(Atom {
                    range: end..atom.range.end,
                    text: source[end..atom.range.end].into(),
                    raw: false,
                });
            }
        }
        replacement.push(Atom {
            range: label_start..end - 2,
            text: source[label_start..end - 2].into(),
            raw: false,
        });
        *atoms = replacement;
        styles.push(StyleSpan {
            source_range: bytes(start..end),
            kind: StyleKind::Link {
                destination: format!("wiki:{}", target.trim()),
                title: target.trim().into(),
            },
        });
        cursor = end;
    }
}

fn extend_tight(tight: &mut Option<Draft>, range: &Range<usize>, context: BlockContext) {
    if let Some(draft) = tight {
        draft.range.start = draft.range.start.min(range.start);
        draft.range.end = draft.range.end.max(range.end);
    } else {
        *tight = Some(Draft {
            range: range.clone(),
            kind: BlockKind::Paragraph,
            context,
        });
    }
}

fn fallback(source: &str, range: Range<usize>) -> Draft {
    let kind = if source[range.clone()].chars().all(char::is_whitespace) {
        BlockKind::Blank
    } else {
        BlockKind::Unsupported
    };
    Draft {
        range,
        kind,
        context: BlockContext::default(),
    }
}

fn line_start(source: &str, at: usize) -> usize {
    source[..at]
        .rfind(['\n', '\r'])
        .map_or(0, |index| index + 1)
}
fn line_end(source: &str, at: usize) -> usize {
    if at == source.len()
        || at > 0
            && (source.as_bytes()[at - 1] == b'\n'
                || source.as_bytes()[at - 1] == b'\r' && source.as_bytes().get(at) != Some(&b'\n'))
    {
        return at;
    }
    if source.as_bytes().get(at) == Some(&b'\r') {
        return at + 1 + usize::from(source.as_bytes().get(at + 1) == Some(&b'\n'));
    }
    if source.as_bytes().get(at) == Some(&b'\n') {
        return at + 1;
    }
    source[at..]
        .find(['\n', '\r'])
        .map_or(source.len(), |index| {
            let end = at + index;
            end + 1
                + usize::from(
                    source.as_bytes()[end] == b'\r'
                        && source.as_bytes().get(end + 1) == Some(&b'\n'),
                )
        })
}

fn code_atoms(source: &str, range: Range<usize>, rendered: &str, atoms: &mut Vec<Atom>) {
    let raw = &source[range.clone()];
    let delimiter = raw.bytes().take_while(|byte| *byte == b'`').count();
    if delimiter == 0 || raw.len() < delimiter * 2 {
        atoms.push(Atom {
            range,
            text: rendered.into(),
            raw: false,
        });
        return;
    }
    let start = range.start + delimiter;
    let end = range.end - delimiter;
    let inner = &source[start..end];
    if inner == rendered {
        atoms.push(Atom {
            range: start..end,
            text: rendered.into(),
            raw: false,
        });
        return;
    }
    if !inner.contains(['\r', '\n'])
        && inner.len() >= 2
        && inner.starts_with(' ')
        && inner.ends_with(' ')
        && &inner[1..inner.len() - 1] == rendered
    {
        atoms.push(Atom {
            range: start + 1..end - 1,
            text: rendered.into(),
            raw: false,
        });
        return;
    }
    let normalized = normalize_line_endings(inner).replace('\n', " ");
    let trim = normalized.starts_with(' ')
        && normalized.ends_with(' ')
        && normalized.chars().any(|ch| ch != ' ');
    let expected = if trim {
        &normalized[1..normalized.len() - 1]
    } else {
        &normalized
    };
    if expected != rendered {
        atoms.push(Atom {
            range,
            text: rendered.into(),
            raw: false,
        });
        return;
    }
    let mut coalesced: Vec<Atom> = Vec::new();
    let mut last_copy = false;
    for (offset, grapheme) in inner.grapheme_indices(true) {
        let mut text: String = if grapheme == "\r\n" || grapheme == "\r" || grapheme == "\n" {
            " ".into()
        } else {
            grapheme.into()
        };
        if trim && offset == 0 {
            text.remove(0);
        }
        if trim && offset + grapheme.len() == inner.len() {
            text.pop();
        }
        if text.is_empty() {
            last_copy = false;
            continue;
        }
        let entry = Atom {
            range: start + offset..start + offset + grapheme.len(),
            text,
            raw: false,
        };
        let is_copy = source[entry.range.clone()] == entry.text;
        if is_copy
            && last_copy
            && let Some(last) = coalesced.last_mut()
            && last.range.end == entry.range.start
        {
            last.range.end = entry.range.end;
            last.text.push_str(&entry.text);
        } else {
            coalesced.push(entry);
        }
        last_copy = is_copy;
    }
    atoms.extend(coalesced);
}

fn text_atoms(source: &str, range: Range<usize>, rendered: &str, atoms: &mut Vec<Atom>) {
    let raw = &source[range.clone()];
    if raw.contains('\r') && normalize_line_endings(raw) == rendered {
        let mut start = range.start;
        for (offset, grapheme) in raw.grapheme_indices(true) {
            if grapheme == "\r\n" || grapheme == "\r" {
                let at = range.start + offset;
                if start < at {
                    atoms.push(Atom {
                        range: start..at,
                        text: source[start..at].into(),
                        raw: false,
                    });
                }
                atoms.push(Atom {
                    range: at..at + grapheme.len(),
                    text: "\n".into(),
                    raw: false,
                });
                start = at + grapheme.len();
            }
        }
        if start < range.end {
            atoms.push(Atom {
                range: start..range.end,
                text: source[start..range.end].into(),
                raw: false,
            });
        }
    } else {
        atoms.push(Atom {
            range,
            text: rendered.into(),
            raw: false,
        });
    }
}

fn normalize_line_endings(source: &str) -> String {
    source.replace("\r\n", "\n").replace('\r', "\n")
}

/// CommonMark line endings are LF, CRLF, or CR. Normalize only the parser's
/// immutable input and translate offsets back through sparse removed-CR deltas.
/// In particular, a CRLF inside inline code is one line ending, not two spaces.
struct CommonMarkInput<'a> {
    text: Cow<'a, str>,
    deltas: Vec<(usize, usize)>,
}
impl<'a> CommonMarkInput<'a> {
    fn new(source: &'a str) -> Self {
        if !source.contains('\r') {
            return Self {
                text: Cow::Borrowed(source),
                deltas: Vec::new(),
            };
        }
        let mut text = String::with_capacity(source.len());
        let mut deltas = Vec::new();
        let mut removed = 0;
        let mut characters = source.chars().peekable();
        while let Some(character) = characters.next() {
            if character == '\r' {
                text.push('\n');
                if characters.peek() == Some(&'\n') {
                    characters.next();
                    removed += 1;
                    deltas.push((text.len(), removed));
                }
            } else {
                text.push(character);
            }
        }
        Self {
            text: Cow::Owned(text),
            deltas,
        }
    }
    fn source_offset(&self, offset: usize) -> usize {
        let index = self.deltas.partition_point(|(at, _)| *at <= offset);
        offset
            + if index == 0 {
                0
            } else {
                self.deltas[index - 1].1
            }
    }
    fn source_range(&self, range: Range<usize>) -> Range<usize> {
        self.source_offset(range.start)..self.source_offset(range.end)
    }
}

#[cfg(test)]
mod resource_tests {
    use super::*;
    use glyphfolio_editor_core::Editor;

    #[test]
    fn long_literal_inline_code_does_not_allocate_an_atom_per_character() {
        let source = format!("` {} `", "x".repeat(100_000));
        let editor = Editor::new(&source);
        let parsed = MarkdownParser::new().parse(&editor.snapshot());
        assert!(parsed.atoms.len() <= 3);
        assert_eq!(
            parsed.project(ProjectionOptions::default()).text(),
            "x".repeat(100_000)
        );
    }
}
