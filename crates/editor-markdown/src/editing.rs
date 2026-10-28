//! Original projection-aware source transactions. Hidden Markdown stays hidden
//! and survives partial visual edits; fully selected inline wrappers are removed.
use crate::{BlockKind, DisplayOffset, Projection, SegmentKind, StyleKind, StyleSpan};
use glyphfolio_editor_core::{
    Affinity, ByteOffset, Editor, Selection, Transaction, TransactionOrigin,
};
use std::ops::Range;

pub fn replace(
    editor: &mut Editor,
    projection: &Projection,
    styles: &[StyleSpan],
    range: Range<usize>,
    inserted: &str,
    origin: TransactionOrigin,
) -> Result<(), String> {
    let snapshot = editor.snapshot();
    let selection = editor.selection();
    validate_styles(snapshot.text().as_str(), styles)?;
    let validation = projection
        .projected_edit(
            &snapshot,
            DisplayOffset(range.start)..DisplayOffset(range.end),
            inserted,
            selection.affinity,
            origin,
        )
        .map_err(|error| format!("{error}. Use Source mode for literal syntax edits."))?;
    if range.is_empty() {
        let at = selection.head;
        if projection.segments().iter().any(|segment| {
            segment.kind == SegmentKind::Copy
                && segment.source_range.start <= at
                && at <= segment.source_range.end
        }) && projection
            .source_to_display(at, selection.affinity)
            .map_err(|error| error.to_string())?
            .0
            == range.start
        {
            return editor
                .insert_text(inserted, origin)
                .map(|_| ())
                .map_err(|error| error.to_string());
        }
        return editor
            .apply(validation)
            .map(|_| ())
            .map_err(|error| error.to_string());
    }
    if selection.start().0 == 0
        && selection.end().0 == snapshot.byte_len()
        && !selection.is_collapsed()
    {
        return editor
            .insert_text(inserted, origin)
            .map(|_| ())
            .map_err(|error| error.to_string());
    }
    let mut deletes: Vec<Range<usize>> = Vec::new();
    for segment in projection.segments() {
        let start = range.start.max(segment.display_range.start.0);
        let end = range.end.min(segment.display_range.end.0);
        if start >= end || segment.kind == SegmentKind::Hidden {
            continue;
        }
        let grid_gap = segment.kind == SegmentKind::Transform
            && projection
                .blocks()
                .iter()
                .filter(|block| {
                    block.kind == BlockKind::Table
                        && block.source_range.start <= segment.source_range.start
                        && segment.source_range.end <= block.source_range.end
                })
                .any(|block| {
                    glyphfolio_editor_core::MarkdownTable::at(
                        projection.source(),
                        block.source_range.start,
                    )
                    .is_some_and(|table| {
                        !table.rows.iter().flat_map(|row| &row.cells).any(|cell| {
                            cell.start <= segment.source_range.start
                                && segment.source_range.end <= cell.end
                        })
                    })
                })
            && matches!(
                &projection.text()[segment.display_range.start.0..segment.display_range.end.0],
                "\t" | "\n"
            );
        if grid_gap {
            continue;
        }
        deletes.push(if segment.kind == SegmentKind::Copy {
            segment.source_range.start.0 + (start - segment.display_range.start.0)
                ..segment.source_range.start.0 + (end - segment.display_range.start.0)
        } else {
            segment.source_range.start.0..segment.source_range.end.0
        });
    }
    for style in styles {
        if !matches!(
            style.kind,
            StyleKind::Strong
                | StyleKind::Emphasis
                | StyleKind::Strikethrough
                | StyleKind::Code
                | StyleKind::Link { .. }
        ) {
            continue;
        }
        let start = projection
            .source_to_display(style.source_range.start, Affinity::After)
            .map_err(|error| error.to_string())?
            .0;
        let end = projection
            .source_to_display(style.source_range.end, Affinity::Before)
            .map_err(|error| error.to_string())?
            .0;
        let inherit = !inserted.is_empty() && start <= range.start && range.start < end;
        if !inherit && start < end && range.start <= start && end <= range.end {
            deletes.push(style.source_range.start.0..style.source_range.end.0);
        }
    }
    for block in projection.blocks() {
        let inherit = !inserted.is_empty()
            && block.display_range.start.0 <= range.start
            && range.start < block.display_range.end.0;
        if !inherit
            && block.display_range.start.0 < block.display_range.end.0
            && range.start <= block.display_range.start.0
            && block.display_range.end.0 <= range.end
        {
            deletes.push(block.source_range.start.0..block.source_range.end.0);
        }
    }
    deletes.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in deletes {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    let at = projection
        .display_to_source(DisplayOffset(range.start), Affinity::After)
        .map_err(|error| error.to_string())?
        .0;
    let at = merged
        .iter()
        .find(|range| range.start <= at && at <= range.end)
        .map_or(at, |range| range.start);
    let mut tx = Transaction::new(&snapshot)
        .with_origin(origin)
        .with_selection(Selection::caret(ByteOffset(at + inserted.len())))
        .with_grapheme_selection(Affinity::After);
    let mut added = false;
    for range in merged {
        let value = if range.start == at {
            added = true;
            inserted
        } else {
            ""
        };
        tx = tx.replace(ByteOffset(range.start)..ByteOffset(range.end), value);
    }
    if !added && !inserted.is_empty() {
        tx = tx.replace(ByteOffset(at)..ByteOffset(at), inserted);
    }
    if !added && inserted.is_empty() {
        return Ok(());
    }
    editor
        .apply(tx)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Plan a structural Enter against disposable snapshots, then submit one minimal
/// source transaction to the real document. No second live document is retained.
pub fn paragraph_break(
    editor: &mut Editor,
    projection: &Projection,
    soft: bool,
) -> Result<Vec<glyphfolio_editor_core::EditCommand>, String> {
    use crate::{MarkdownDialect, MarkdownParser, ProjectionOptions};
    use glyphfolio_editor_core::EditCommand;
    let original = editor.snapshot();
    if projection.document_id() != original.document_id()
        || projection.revision() != original.revision()
    {
        return Err("Rich projection belongs to a different document revision".into());
    }
    let source = original.text();
    let selected = editor.selection();
    let mut plan = Editor::new(&source);
    plan.set_selection(selected)
        .map_err(|error| error.to_string())?;
    let mut parser = MarkdownParser::with_dialect(MarkdownDialect::GfmWiki);
    if !selected.is_collapsed() {
        let parsed = parser.parse(&plan.snapshot());
        let view = parsed.project(ProjectionOptions::default());
        let start = projection
            .source_to_display(selected.start(), Affinity::After)
            .map_err(|error| error.to_string())?
            .0;
        let end = projection
            .source_to_display(selected.end(), Affinity::Before)
            .map_err(|error| error.to_string())?
            .0;
        replace(
            &mut plan,
            &view,
            parsed.styles(),
            start..end,
            "",
            TransactionOrigin::Delete,
        )?;
    } else if !projection.segments().iter().any(|segment| {
        segment.kind == SegmentKind::Copy
            && segment.source_range.start <= selected.head
            && selected.head <= segment.source_range.end
    }) {
        let display = projection
            .source_to_display(selected.head, selected.affinity)
            .map_err(|error| error.to_string())?;
        let at = projection
            .display_to_source(display, selected.affinity)
            .map_err(|error| error.to_string())?;
        plan.set_selection(Selection::caret(at))
            .map_err(|error| error.to_string())?;
    }
    let parsed = parser.parse(&plan.snapshot());
    let view = parsed.project(ProjectionOptions::default());
    let before = plan.snapshot().text();
    let at = plan.selection().head.0;
    let display = view
        .source_to_display(ByteOffset(at), Affinity::After)
        .map_err(|error| error.to_string())?
        .0;
    let mut point = at;
    let mut original_offset = 0;
    let mut wrappers = Vec::new();
    let mut pending = Vec::new();
    for span in parsed.styles() {
        if span.source_range.start.0 >= at || at >= span.source_range.end.0 {
            continue;
        }
        let raw = &before[span.source_range.start.0..span.source_range.end.0];
        let (open, close, command) = match &span.kind {
            StyleKind::Strong => (
                raw[..2].to_owned(),
                raw[raw.len() - 2..].to_owned(),
                Some(EditCommand::ToggleStrong),
            ),
            StyleKind::Emphasis => (
                raw[..1].to_owned(),
                raw[raw.len() - 1..].to_owned(),
                Some(EditCommand::ToggleEmphasis),
            ),
            StyleKind::Strikethrough => {
                let width = raw
                    .bytes()
                    .take_while(|byte| *byte == b'~')
                    .count()
                    .clamp(1, 2);
                (
                    raw[..width].to_owned(),
                    raw[raw.len() - width..].to_owned(),
                    Some(EditCommand::ToggleStrikethrough),
                )
            }
            StyleKind::Code => {
                let width = raw.bytes().take_while(|byte| *byte == b'`').count();
                if width == 0 {
                    continue;
                }
                let content = &raw[width..raw.len() - width];
                let padding = usize::from(
                    content.starts_with(' ')
                        && content.ends_with(' ')
                        && !content.trim().is_empty(),
                );
                (
                    raw[..width + padding].to_owned(),
                    raw[raw.len() - width - padding..].to_owned(),
                    Some(EditCommand::ToggleInlineCode),
                )
            }
            StyleKind::Link { .. } => {
                if let Some(label) = raw_label(raw) {
                    ("[".into(), raw[label.len() + 1..].to_owned(), None)
                } else {
                    point = point.max(span.source_range.end.0);
                    continue;
                }
            }
            _ => continue,
        };
        let first = view
            .source_to_display(span.source_range.start, Affinity::After)
            .map_err(|error| error.to_string())?
            .0;
        let last = view
            .source_to_display(span.source_range.end, Affinity::Before)
            .map_err(|error| error.to_string())?
            .0;
        if display == first {
            point = point.min(span.source_range.start.0);
            original_offset = at - point;
        } else if display == last {
            point = point.max(span.source_range.end.0);
            if let Some(command) = command {
                pending.push(command);
            }
        } else if first < display && display < last {
            wrappers.push((
                span.source_range.start.0,
                span.source_range.end.0,
                open,
                close,
            ));
        }
    }
    wrappers.sort_by_key(|(start, end, _, _)| (*start, std::cmp::Reverse(*end)));
    let opening: String = wrappers
        .iter()
        .map(|(_, _, open, _)| open.as_str())
        .collect();
    let closing: String = wrappers
        .iter()
        .rev()
        .map(|(_, _, _, close)| close.as_str())
        .collect();
    plan.set_selection(Selection::caret(ByteOffset(point)))
        .map_err(|error| error.to_string())?;
    let change = plan
        .execute(EditCommand::Enter)
        .map_err(|error| error.to_string())?;
    if let Some(change) = change
        && change.changes.len() == 1
        && change.changes[0].range.is_empty()
    {
        let inserted = &change.changes[0].inserted;
        let block = parsed
            .blocks()
            .iter()
            .find(|block| block.source_range.start.0 <= at && at <= block.source_range.end.0);
        let ordinary = block.is_none_or(|block| {
            matches!(
                block.kind,
                BlockKind::Paragraph | BlockKind::Blank | BlockKind::Unsupported
            ) && block.context.list_depth == 0
                && block.context.quote_depth == 0
        });
        let break_text = if soft {
            format!(
                "  {}",
                if inserted.starts_with("\r\n") {
                    "\r\n"
                } else {
                    "\n"
                }
            )
        } else if ordinary {
            format!("{inserted}{inserted}")
        } else {
            inserted.clone()
        };
        let value = format!("{closing}{break_text}{opening}");
        let caret = point + closing.len() + break_text.len() + opening.len() + original_offset;
        plan.apply(
            Transaction::new(&plan.snapshot())
                .replace(ByteOffset(point)..ByteOffset(point + inserted.len()), value)
                .with_selection(Selection::caret(ByteOffset(caret)))
                .with_grapheme_selection(Affinity::After),
        )
        .map_err(|error| error.to_string())?;
    }
    let after = plan.snapshot().text();
    let mut prefix = source
        .bytes()
        .zip(after.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !source.is_char_boundary(prefix) || !after.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let mut suffix = source[prefix..]
        .bytes()
        .rev()
        .zip(after[prefix..].bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();
    while !source.is_char_boundary(source.len() - suffix)
        || !after.is_char_boundary(after.len() - suffix)
    {
        suffix -= 1;
    }
    editor
        .apply(
            Transaction::new(&original)
                .replace(
                    ByteOffset(prefix)..ByteOffset(source.len() - suffix),
                    &after[prefix..after.len() - suffix],
                )
                .with_selection(plan.selection())
                .with_origin(TransactionOrigin::Typing)
                .with_grapheme_selection(Affinity::After),
        )
        .map_err(|error| error.to_string())?;
    Ok(pending)
}

/// Join adjacent text blocks without deleting hidden syntax or exposing adjacent
/// runs such as `**left****right**`. Structural blocks keep their own commands.
pub fn join_backward(
    editor: &mut Editor,
    projection: &Projection,
    styles: &[StyleSpan],
) -> Result<Option<bool>, String> {
    let snapshot = editor.snapshot();
    if projection.document_id() != snapshot.document_id()
        || projection.revision() != snapshot.revision()
    {
        return Err("Rich projection belongs to a different document revision".into());
    }
    validate_styles(&snapshot.text(), styles)?;
    let selected = editor.selection();
    if !selected.is_collapsed() {
        return Ok(None);
    }
    let display = projection
        .source_to_display(selected.head, selected.affinity)
        .map_err(|error| error.to_string())?
        .0;
    let Some(index) = projection.blocks().iter().position(|block| {
        block.display_range.start.0 == display
            && block.source_range.start <= selected.head
            && selected.head <= block.source_range.end
    }) else {
        return Ok(None);
    };
    let current = &projection.blocks()[index];
    if current.kind != BlockKind::Paragraph
        || current.context.list_depth > 0
        || current.context.quote_depth > 0
    {
        return Ok(None);
    }
    let Some(previous) = projection.blocks()[..index]
        .iter()
        .rev()
        .find(|block| block.kind != BlockKind::Blank)
    else {
        return Ok(Some(false));
    };
    if !matches!(previous.kind, BlockKind::Paragraph | BlockKind::Heading(_))
        || previous.context.list_depth > 0
        || previous.context.quote_depth > 0
    {
        return Ok(Some(false));
    }
    let snapshot = editor.snapshot();
    let source = snapshot.text();
    let previous_end = previous.source_range.start.0
        + source[previous.source_range.start.0..previous.source_range.end.0]
            .trim_end_matches(char::is_whitespace)
            .len();
    let mut current_start = projection
        .display_to_source(current.display_range.start, Affinity::After)
        .map_err(|error| error.to_string())?
        .0;
    let mut previous_display = previous.display_range.end.0;
    while previous_display > previous.display_range.start.0
        && projection.text()[..previous_display]
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace)
    {
        previous_display -= projection.text()[..previous_display]
            .chars()
            .next_back()
            .unwrap()
            .len_utf8();
    }
    let leading: Vec<_> = styles
        .iter()
        .filter(|span| {
            current.source_range.start <= span.source_range.start
                && span.source_range.end <= current.source_range.end
                && projection
                    .source_to_display(span.source_range.start, Affinity::After)
                    .is_ok_and(|offset| offset.0 == display)
        })
        .collect();
    if let Some(first) = leading.iter().map(|span| span.source_range.start.0).min() {
        current_start = current_start.min(first);
    }
    let trailing: Vec<_> = styles
        .iter()
        .filter(|span| {
            previous.source_range.start <= span.source_range.start
                && span.source_range.end.0 <= previous_end
                && projection
                    .source_to_display(span.source_range.end, Affinity::Before)
                    .is_ok_and(|offset| offset.0 == previous_display)
        })
        .collect();
    let mut deletes: Vec<Range<usize>> = std::iter::once(previous_end..current_start).collect();
    let mut caret = previous_end;
    for left in trailing {
        let Some((_, close)) = inline_delimiters(
            &left.kind,
            &source[left.source_range.start.0..left.source_range.end.0],
        ) else {
            continue;
        };
        let right = leading.iter().find(|right| {
            right.kind == left.kind
                && inline_delimiters(
                    &right.kind,
                    &source[right.source_range.start.0..right.source_range.end.0],
                )
                .is_some_and(|(open, _)| open == close)
        });
        if let Some(right) = right {
            let start = left.source_range.end.0 - close.len();
            caret = caret.min(start);
            deletes.push(start..left.source_range.end.0);
            deletes.push(right.source_range.start.0..right.source_range.start.0 + close.len());
        }
    }
    deletes.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in deletes {
        if range.is_empty() {
            continue;
        }
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    if merged.is_empty() {
        return Ok(Some(false));
    }
    let mut tx = Transaction::new(&snapshot)
        .with_selection(Selection::caret(ByteOffset(caret)))
        .with_grapheme_selection(Affinity::After)
        .with_origin(TransactionOrigin::Delete);
    for range in merged {
        tx = tx.replace(ByteOffset(range.start)..ByteOffset(range.end), "");
    }
    editor.apply(tx).map_err(|error| error.to_string())?;
    Ok(Some(true))
}
fn inline_delimiters(kind: &StyleKind, raw: &str) -> Option<(String, String)> {
    let width = match kind {
        StyleKind::Strong => 2,
        StyleKind::Emphasis => 1,
        StyleKind::Strikethrough => raw
            .bytes()
            .take_while(|byte| *byte == b'~')
            .count()
            .clamp(1, 2),
        StyleKind::Code => raw.bytes().take_while(|byte| *byte == b'`').count(),
        StyleKind::Link { .. } => {
            let label = raw_label(raw)?;
            return Some(("[".into(), raw[label.len() + 1..].into()));
        }
        _ => return None,
    };
    let byte = *raw.as_bytes().first()?;
    if width == 0
        || raw.len() < width * 2
        || !matches!(byte, b'*' | b'_' | b'~' | b'`')
        || !raw.as_bytes()[..width].iter().all(|value| *value == byte)
        || !raw.as_bytes()[raw.len() - width..]
            .iter()
            .all(|value| *value == byte)
    {
        return None;
    }
    Some((raw[..width].into(), raw[raw.len() - width..].into()))
}

fn raw_label(source: &str) -> Option<&str> {
    if !source.starts_with('[') {
        return None;
    }
    let mut depth = 1usize;
    let mut at = 1;
    let bytes = source.as_bytes();
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => {
                at += 2;
                continue;
            }
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&source[1..at]);
                }
            }
            _ => {}
        }
        at += 1;
    }
    None
}

fn validate_styles(source: &str, styles: &[StyleSpan]) -> Result<(), String> {
    if styles.iter().any(|span| {
        span.source_range.start > span.source_range.end
            || span.source_range.end.0 > source.len()
            || !source.is_char_boundary(span.source_range.start.0)
            || !source.is_char_boundary(span.source_range.end.0)
    }) {
        Err("Invalid Markdown style source range".into())
    } else {
        Ok(())
    }
}
