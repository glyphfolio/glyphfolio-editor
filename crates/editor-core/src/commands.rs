//! Original source commands. These are deliberate Markdown source operations,
//! not an AST serializer: unrelated spelling and line endings remain untouched.

use std::ops::Range;

use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    Affinity, ByteOffset, ChangeMap, CommittedChange, Editor, EditorError, Selection, TextChange,
    Transaction, TransactionOrigin,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadingLevel {
    One,
    Two,
    Three,
    Four,
    Five,
    Six,
}

impl HeadingLevel {
    pub fn number(self) -> u8 {
        match self {
            Self::One => 1,
            Self::Two => 2,
            Self::Three => 3,
            Self::Four => 4,
            Self::Five => 5,
            Self::Six => 6,
        }
    }
}

/// Commands shared by toolbar, keyboard, CLI, and alternative UI hosts.
/// Empty inline formatting inserts a delimiter pair with the caret inside it;
/// it does not introduce an invisible stored-formatting state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditCommand {
    ToggleStrong,
    ToggleStrikethrough,
    ToggleTaskList,
    ToggleTaskChecked,
    Table(crate::TableCommand),
    ToggleEmphasis,
    ToggleInlineCode,
    SetHeading(Option<HeadingLevel>),
    ToggleUnorderedList,
    ToggleOrderedList,
    ToggleBlockQuote,
    ToggleCodeFence,
    Indent,
    Outdent,
    Enter,
    Backspace,
}

#[derive(Clone, Debug)]
struct Line {
    start: usize,
    content_end: usize,
    end: usize,
}

impl Line {
    fn ending<'a>(&self, text: &'a str) -> &'a str {
        &text[self.content_end..self.end]
    }
    fn base(&self, text: &str) -> usize {
        self.start
            + usize::from(self.start == 0 && text.starts_with('\u{feff}')) * '\u{feff}'.len_utf8()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListKind {
    Unordered,
    Ordered,
}

#[derive(Clone, Debug)]
struct ListMarker {
    range: Range<usize>,
    kind: ListKind,
    digits: usize,
    number: u32,
    delimiter: u8,
    padding: String,
}

#[derive(Clone, Debug)]
struct Prefix {
    base: usize,
    indent_end: usize,
    marker_start: usize,
    quotes: Vec<Range<usize>>,
    list: Option<ListMarker>,
    heading: Option<Range<usize>>,
    content: usize,
}

fn lines(text: &str) -> Vec<Line> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut at = 0;
    while at < text.len() {
        let width = match text.as_bytes()[at] {
            b'\r' if text.as_bytes().get(at + 1) == Some(&b'\n') => 2,
            b'\r' | b'\n' => 1,
            _ => {
                at += 1;
                continue;
            }
        };
        result.push(Line {
            start,
            content_end: at,
            end: at + width,
        });
        at += width;
        start = at;
    }
    result.push(Line {
        start,
        content_end: text.len(),
        end: text.len(),
    });
    result
}

fn line_at(lines: &[Line], offset: usize) -> usize {
    lines
        .partition_point(|line| line.start <= offset)
        .saturating_sub(1)
}

fn selected_lines(lines: &[Line], selection: Selection) -> Range<usize> {
    let first = line_at(lines, selection.start().0);
    let last = line_at(
        lines,
        if selection.is_collapsed() {
            selection.end().0
        } else {
            selection.end().0.saturating_sub(1)
        },
    );
    first..last + 1
}

fn whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t')
}

fn skip_space(text: &str, mut at: usize, end: usize) -> usize {
    while at < end && whitespace(text.as_bytes()[at]) {
        at += 1;
    }
    at
}

fn prefix(text: &str, line: &Line) -> Prefix {
    let base = line.base(text);
    let indent_end = skip_space(text, base, line.content_end);
    let mut at = indent_end;
    let mut quotes = Vec::new();
    while text.as_bytes().get(at) == Some(&b'>') && at < line.content_end {
        let start = at;
        at += 1;
        if at < line.content_end && whitespace(text.as_bytes()[at]) {
            at += 1;
        }
        quotes.push(start..at);
        at = skip_space(text, at, line.content_end);
    }
    let marker_start = at;
    let list = list_marker(text, at, line.content_end);
    if let Some(marker) = &list {
        at = marker.range.end;
    }
    let heading = heading_marker(text, at, line.content_end);
    if let Some(marker) = &heading {
        at = marker.end;
    }
    Prefix {
        base,
        indent_end,
        marker_start,
        quotes,
        list,
        heading,
        content: at,
    }
}

fn list_marker(text: &str, start: usize, end: usize) -> Option<ListMarker> {
    let bytes = text.as_bytes();
    let first = *bytes.get(start)?;
    if start >= end {
        return None;
    }
    let (kind, mut at, digits, number, delimiter) = if matches!(first, b'-' | b'+' | b'*') {
        (ListKind::Unordered, start + 1, 0, 0, first)
    } else if first.is_ascii_digit() {
        let mut at = start;
        while at < end && bytes[at].is_ascii_digit() && at - start < 10 {
            at += 1;
        }
        if at - start > 9 || !matches!(bytes.get(at), Some(b'.' | b')')) {
            return None;
        }
        (
            ListKind::Ordered,
            at + 1,
            at - start,
            text[start..at].parse().ok()?,
            bytes[at],
        )
    } else {
        return None;
    };
    if at < end && !whitespace(bytes[at]) {
        return None;
    }
    let padding_start = at;
    if at < end && bytes[at] == b'\t' {
        at += 1;
    } else {
        let all = skip_space(text, at, end);
        at = if all - at > 4 { at + 1 } else { all };
    }
    Some(ListMarker {
        range: start..at,
        kind,
        digits,
        number,
        delimiter,
        padding: text[padding_start..at].into(),
    })
}

fn heading_marker(text: &str, start: usize, end: usize) -> Option<Range<usize>> {
    let mut at = start;
    while at < end && text.as_bytes()[at] == b'#' {
        at += 1;
    }
    if !(1..=6).contains(&(at - start)) || (at < end && !whitespace(text.as_bytes()[at])) {
        return None;
    }
    Some(start..skip_space(text, at, end))
}

fn legal_selection(text: &str, selection: Selection) -> Selection {
    let mut start = selection.start().0;
    let mut end = selection.end().0;
    for (at, value) in text.grapheme_indices(true) {
        let after = at + value.len();
        if selection.is_collapsed() && at < start && start < after {
            start = if selection.affinity == Affinity::Before {
                at
            } else {
                after
            };
            end = start;
            break;
        }
        if at < start && start < after {
            start = at;
        }
        if at < end && end < after {
            end = after;
        }
    }
    Selection {
        anchor: ByteOffset(if selection.anchor <= selection.head {
            start
        } else {
            end
        }),
        head: ByteOffset(if selection.anchor <= selection.head {
            end
        } else {
            start
        }),
        affinity: selection.affinity,
    }
}

fn change(range: Range<usize>, inserted: impl Into<String>) -> TextChange {
    TextChange::new(ByteOffset(range.start)..ByteOffset(range.end), inserted)
}

fn mapped_selection(
    text: &str,
    changes: &[TextChange],
    selection: Selection,
    exclude_outer_insertions: bool,
) -> Result<Selection, EditorError> {
    let map = ChangeMap {
        before_bytes: text.len(),
        changes: changes.to_vec(),
    };
    let map_endpoint = |offset: ByteOffset| {
        let affinity =
            if exclude_outer_insertions && offset == selection.end() && !selection.is_collapsed() {
                Affinity::Before
            } else {
                Affinity::After
            };
        map.map_offset(offset, affinity)
    };
    Ok(Selection {
        anchor: map_endpoint(selection.anchor)?,
        head: map_endpoint(selection.head)?,
        affinity: selection.affinity,
    })
}

fn commit(
    editor: &mut Editor,
    text: &str,
    mut changes: Vec<TextChange>,
    selection: Selection,
    explicit: Option<Selection>,
    inline: bool,
    origin: TransactionOrigin,
) -> Result<Option<CommittedChange>, EditorError> {
    changes.retain(|item| text[item.range.start.0..item.range.end.0] != item.inserted);
    if changes.is_empty() {
        return Ok(None);
    }
    changes.sort_by_key(|item| (item.range.start, item.range.end));
    let after = match explicit {
        Some(after) => after,
        None => mapped_selection(text, &changes, selection, inline)?,
    };
    let mut transaction = Transaction::new(&editor.snapshot())
        .with_origin(origin)
        .with_selection(after);
    for item in changes {
        transaction = transaction.replace(item.range, item.inserted);
    }
    editor.apply(transaction).map(Some)
}

pub(super) fn execute(
    editor: &mut Editor,
    command: EditCommand,
) -> Result<Option<CommittedChange>, EditorError> {
    let text = editor.snapshot().text();
    let selection = legal_selection(&text, editor.selection());
    match command {
        EditCommand::Table(command) => editor.edit_table(command),
        EditCommand::ToggleStrikethrough => strike(editor, &text, selection),
        EditCommand::ToggleTaskList => tasks(editor, &text, selection, false),
        EditCommand::ToggleTaskChecked => tasks(editor, &text, selection, true),
        EditCommand::ToggleStrong => inline_mark(editor, &text, selection, true),
        EditCommand::ToggleEmphasis => inline_mark(editor, &text, selection, false),
        EditCommand::ToggleInlineCode => inline_code(editor, &text, selection),
        EditCommand::SetHeading(level) => heading(editor, &text, selection, level),
        EditCommand::ToggleUnorderedList => list(editor, &text, selection, ListKind::Unordered),
        EditCommand::ToggleOrderedList => list(editor, &text, selection, ListKind::Ordered),
        EditCommand::ToggleBlockQuote => quote(editor, &text, selection),
        EditCommand::ToggleCodeFence => fence(editor, &text, selection),
        EditCommand::Indent | EditCommand::Outdent => {
            indent(editor, &text, selection, command == EditCommand::Indent)
        }
        EditCommand::Enter => enter(editor, &text, selection),
        EditCommand::Backspace => backspace(editor, &text, selection),
    }
}

fn escaped(text: &str, at: usize) -> bool {
    text.as_bytes()[..at]
        .iter()
        .rev()
        .take_while(|&&byte| byte == b'\\')
        .count()
        % 2
        == 1
}

fn run_before(text: &str, at: usize, byte: u8) -> usize {
    text.as_bytes()[..at]
        .iter()
        .rev()
        .take_while(|&&value| value == byte)
        .count()
}
fn run_after(text: &str, at: usize, byte: u8) -> usize {
    text.as_bytes()[at..]
        .iter()
        .take_while(|&&value| value == byte)
        .count()
}

fn markdown_whitespace(value: Option<char>) -> bool {
    value.is_none_or(|value| {
        matches!(value, '\t' | '\n' | '\u{000c}' | '\r')
            || get_general_category(value) == GeneralCategory::SpaceSeparator
    })
}

fn markdown_punctuation(value: Option<char>) -> bool {
    value.is_some_and(|value| {
        use GeneralCategory::*;
        matches!(
            get_general_category(value),
            ClosePunctuation
                | ConnectorPunctuation
                | DashPunctuation
                | FinalPunctuation
                | InitialPunctuation
                | OpenPunctuation
                | OtherPunctuation
                | CurrencySymbol
                | MathSymbol
                | ModifierSymbol
                | OtherSymbol
        )
    })
}

/// Original lexical guards from CommonMark 0.31.2 section 6.2. Source commands
/// do not own a parser: ambiguous nested candidates are preserved, not stripped.
/// https://spec.commonmark.org/0.31.2/#emphasis-and-strong-emphasis
fn delimiter_flanks(text: &str, run: Range<usize>) -> (bool, bool) {
    let before = text[..run.start].chars().next_back();
    let after = text[run.end..].chars().next();
    let before_space = markdown_whitespace(before);
    let after_space = markdown_whitespace(after);
    let before_punctuation = markdown_punctuation(before);
    let after_punctuation = markdown_punctuation(after);
    let left = !after_space && (!after_punctuation || before_space || before_punctuation);
    let right = !before_space && (!before_punctuation || after_space || after_punctuation);
    if text.as_bytes()[run.start] == b'_' {
        (
            left && (!right || before_punctuation),
            right && (!left || after_punctuation),
        )
    } else {
        (left, right)
    }
}

fn mark_pair_is_unambiguous(
    text: &str,
    open: Range<usize>,
    close: Range<usize>,
    strong: bool,
) -> bool {
    if open.end > close.start || escaped(text, open.start) || escaped(text, close.start) {
        return false;
    }
    let (can_open, open_can_close) = delimiter_flanks(text, open.clone());
    let (close_can_open, can_close) = delimiter_flanks(text, close.clone());
    if !can_open
        || !can_close
        || ((open_can_close || close_can_open)
            && (open.len() + close.len()).is_multiple_of(3)
            && (!open.len().is_multiple_of(3) || !close.len().is_multiple_of(3)))
    {
        return false;
    }
    let marker = text.as_bytes()[open.start];
    let mut at = open.end;
    while at < close.start {
        if text.as_bytes()[at] == marker && !escaped(text, at) {
            let count = run_after(text, at, marker).min(close.start - at);
            if if strong { count >= 2 } else { count % 2 == 1 } {
                return false;
            }
            at += count;
        } else {
            at += 1;
        }
    }
    true
}

fn mark_envelope(
    text: &str,
    range: &Range<usize>,
    strong: bool,
) -> Option<(Range<usize>, Range<usize>)> {
    let length = if strong { 2 } else { 1 };
    let eligible = |count: usize| if strong { count >= 2 } else { count % 2 == 1 };
    for byte in *b"*_" {
        let before = run_before(text, range.start, byte);
        let after = run_after(text, range.end, byte);
        if eligible(before)
            && eligible(after)
            && mark_pair_is_unambiguous(
                text,
                range.start - before..range.start,
                range.end..range.end + after,
                strong,
            )
        {
            return Some((
                range.start - length..range.start,
                range.end..range.end + length,
            ));
        }
        let open = run_after(text, range.start, byte).min(range.len());
        let close = run_before(text, range.end, byte).min(range.len());
        if eligible(open)
            && eligible(close)
            && range.len() >= length * 2
            && mark_pair_is_unambiguous(
                text,
                range.start..range.start + open,
                range.end - close..range.end,
                strong,
            )
        {
            return Some((
                range.start..range.start + length,
                range.end - length..range.end,
            ));
        }
    }
    None
}

fn trimmed_range(text: &str, range: Range<usize>) -> Option<Range<usize>> {
    let value = &text[range.clone()];
    let left = value.len() - value.trim_start_matches(char::is_whitespace).len();
    let right = value.trim_end_matches(char::is_whitespace).len();
    (left < right).then_some(range.start + left..range.start + right)
}

fn inline_mark(
    editor: &mut Editor,
    text: &str,
    selection: Selection,
    strong: bool,
) -> Result<Option<CommittedChange>, EditorError> {
    let marker = if strong { "**" } else { "*" };
    if selection.is_collapsed() {
        let at = selection.head.0;
        return commit(
            editor,
            text,
            vec![change(at..at, marker.repeat(2))],
            selection,
            Some(Selection::caret(ByteOffset(at + marker.len()))),
            true,
            TransactionOrigin::Formatting,
        );
    }
    let selected = selection.start().0..selection.end().0;
    let ranges: Vec<_> = lines(text)
        .iter()
        .filter_map(|line| {
            let start = line.start.max(selected.start);
            let end = line.content_end.min(selected.end);
            (start < end)
                .then(|| trimmed_range(text, start..end))
                .flatten()
        })
        .collect();
    let envelopes: Vec<_> = ranges
        .iter()
        .map(|range| mark_envelope(text, range, strong))
        .collect();
    let remove = !envelopes.is_empty() && envelopes.iter().all(Option::is_some);
    let mut changes = Vec::new();
    for (range, envelope) in ranges.into_iter().zip(envelopes) {
        if remove {
            let (open, close) = envelope.expect("all selected spans contain marks");
            changes.extend([change(open, ""), change(close, "")]);
        } else if envelope.is_none() {
            changes.extend([
                change(range.start..range.start, marker),
                change(range.end..range.end, marker),
            ]);
        }
    }
    commit(
        editor,
        text,
        changes,
        selection,
        None,
        true,
        TransactionOrigin::Formatting,
    )
}

fn max_backticks(value: &str) -> usize {
    value
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0)
}

fn has_backtick_run(value: &str, length: usize) -> bool {
    value
        .split(|character| character != '`')
        .any(|run| run.len() == length)
}

fn code_envelope(text: &str, range: &Range<usize>) -> Option<(Range<usize>, Range<usize>)> {
    let before = run_before(text, range.start, b'`');
    let after = run_after(text, range.end, b'`');
    if before > 0
        && before == after
        && !escaped(text, range.start - before)
        && !has_backtick_run(&text[range.clone()], before)
    {
        return Some((
            range.start - before..range.start,
            range.end..range.end + after,
        ));
    }
    if range.start > 0
        && range.end < text.len()
        && text.as_bytes()[range.start - 1] == b' '
        && text.as_bytes()[range.end] == b' '
    {
        let before = run_before(text, range.start - 1, b'`');
        let after = run_after(text, range.end + 1, b'`');
        if before > 0
            && before == after
            && !escaped(text, range.start - 1 - before)
            && !has_backtick_run(&text[range.clone()], before)
        {
            return Some((
                range.start - before - 1..range.start,
                range.end..range.end + after + 1,
            ));
        }
    }
    let open = run_after(text, range.start, b'`').min(range.len());
    let close = run_before(text, range.end, b'`').min(range.len());
    if open == 0 || open != close || range.len() < open * 2 || escaped(text, range.start) {
        return None;
    }
    let content = &text[range.start + open..range.end - close];
    if has_backtick_run(content, open) {
        return None;
    }
    let pad = usize::from(
        content.starts_with(' ')
            && content.ends_with(' ')
            && !content.chars().all(|character| character == ' '),
    );
    Some((
        range.start..range.start + open + pad,
        range.end - close - pad..range.end,
    ))
}

fn inline_code(
    editor: &mut Editor,
    text: &str,
    selection: Selection,
) -> Result<Option<CommittedChange>, EditorError> {
    let range = selection.start().0..selection.end().0;
    if !range.is_empty()
        && let Some((open, close)) = code_envelope(text, &range)
    {
        return commit(
            editor,
            text,
            vec![change(open, ""), change(close, "")],
            selection,
            None,
            true,
            TransactionOrigin::Formatting,
        );
    }
    let value = &text[range.clone()];
    let delimiter = "`".repeat(max_backticks(value) + 1);
    let padded = value.starts_with('`')
        || value.ends_with('`')
        || (value.starts_with(' ')
            && value.ends_with(' ')
            && !value.chars().all(|character| character == ' '));
    let open = format!("{delimiter}{}", if padded { " " } else { "" });
    let close = format!("{}{delimiter}", if padded { " " } else { "" });
    if range.is_empty() {
        return commit(
            editor,
            text,
            vec![change(range.clone(), format!("{open}{close}"))],
            selection,
            Some(Selection::caret(ByteOffset(range.start + open.len()))),
            true,
            TransactionOrigin::Formatting,
        );
    }
    commit(
        editor,
        text,
        vec![
            change(range.start..range.start, open),
            change(range.end..range.end, close),
        ],
        selection,
        None,
        true,
        TransactionOrigin::Formatting,
    )
}

fn heading(
    editor: &mut Editor,
    text: &str,
    selection: Selection,
    level: Option<HeadingLevel>,
) -> Result<Option<CommittedChange>, EditorError> {
    let lines = lines(text);
    let target = selected_lines(&lines, selection);
    let mut changes = Vec::new();
    let mut skipped = Vec::new();
    for index in target {
        if skipped.contains(&index) {
            continue;
        }
        let mut index = index;
        if index > 0 && setext_pair(text, &lines[index - 1], &lines[index]) {
            index -= 1;
        }
        let line = &lines[index];
        let prefix = prefix(text, line);
        let replacement = level
            .map(|level| format!("{} ", "#".repeat(level.number() as usize)))
            .unwrap_or_default();
        if let Some(marker) = prefix.heading.clone() {
            changes.push(change(marker, replacement));
        } else if level.is_some() {
            changes.push(change(prefix.content..prefix.content, replacement));
        }
        if index + 1 < lines.len() && setext_pair(text, line, &lines[index + 1]) {
            let underline = &lines[index + 1];
            let start = if underline.end == text.len() && underline.ending(text).is_empty() {
                line.content_end
            } else {
                underline.start
            };
            changes.push(change(start..underline.end, ""));
            skipped.push(index + 1);
        }
        if level.is_none() && prefix.heading.is_some() {
            let value = &text[prefix.content..line.content_end];
            let trimmed = value.trim_end_matches([' ', '\t']);
            let hashes = trimmed
                .as_bytes()
                .iter()
                .rev()
                .take_while(|&&value| value == b'#')
                .count();
            if hashes > 0
                && trimmed.len() > hashes
                && whitespace(trimmed.as_bytes()[trimmed.len() - hashes - 1])
            {
                let end = trimmed.len() - hashes;
                let start = value[..end].trim_end_matches([' ', '\t']).len();
                changes.push(change(prefix.content + start..line.content_end, ""));
            }
        }
    }
    changes.dedup_by(|left, right| left.range == right.range && left.inserted == right.inserted);
    commit(
        editor,
        text,
        changes,
        selection,
        None,
        false,
        TransactionOrigin::Formatting,
    )
}

fn is_setext(text: &str, line: &Line) -> bool {
    let source = &text[line.base(text)..line.content_end];
    let indent = source.bytes().take_while(|&byte| byte == b' ').count();
    if indent > 3 {
        return false;
    }
    let value = source[indent..].trim_end_matches([' ', '\t']);
    !value.is_empty()
        && (value.bytes().all(|byte| byte == b'=') || value.bytes().all(|byte| byte == b'-'))
}

fn setext_pair(text: &str, previous: &Line, underline: &Line) -> bool {
    let prefix = prefix(text, previous);
    let value = &text[prefix.content..previous.content_end];
    let thematic = b"-*_".iter().any(|&marker| {
        let stripped: Vec<_> = value.bytes().filter(|byte| !whitespace(*byte)).collect();
        stripped.len() >= 3 && stripped.iter().all(|byte| *byte == marker)
    });
    is_setext(text, underline)
        && !value.trim().is_empty()
        && prefix.heading.is_none()
        && prefix.list.is_none()
        && prefix.quotes.is_empty()
        && prefix.indent_end - prefix.base <= 3
        && !thematic
}

fn list(
    editor: &mut Editor,
    text: &str,
    selection: Selection,
    kind: ListKind,
) -> Result<Option<CommittedChange>, EditorError> {
    let lines = lines(text);
    let target: Vec<_> = selected_lines(&lines, selection)
        .map(|index| (&lines[index], prefix(text, &lines[index])))
        .collect();
    let remove = target.iter().all(|(_, prefix)| {
        prefix
            .list
            .as_ref()
            .is_some_and(|marker| marker.kind == kind)
    });
    let mut changes = Vec::new();
    for (index, (_, prefix)) in target.iter().enumerate() {
        let range = prefix
            .list
            .as_ref()
            .map(|marker| marker.range.clone())
            .unwrap_or(prefix.marker_start..prefix.marker_start);
        let inserted = if remove {
            String::new()
        } else if kind == ListKind::Unordered {
            "- ".into()
        } else {
            format!("{}. ", index + 1)
        };
        changes.push(change(range, inserted));
    }
    commit(
        editor,
        text,
        changes,
        selection,
        None,
        false,
        TransactionOrigin::Formatting,
    )
}

fn quote(
    editor: &mut Editor,
    text: &str,
    selection: Selection,
) -> Result<Option<CommittedChange>, EditorError> {
    let lines = lines(text);
    let target: Vec<_> = selected_lines(&lines, selection)
        .map(|index| prefix(text, &lines[index]))
        .collect();
    let remove = target.iter().all(|prefix| !prefix.quotes.is_empty());
    let changes = target
        .into_iter()
        .filter_map(|prefix| {
            if remove {
                Some(change(prefix.quotes[0].clone(), ""))
            } else if prefix.quotes.is_empty() {
                Some(change(prefix.indent_end..prefix.indent_end, "> "))
            } else {
                None
            }
        })
        .collect();
    commit(
        editor,
        text,
        changes,
        selection,
        None,
        false,
        TransactionOrigin::Formatting,
    )
}

fn indent(
    editor: &mut Editor,
    text: &str,
    selection: Selection,
    add: bool,
) -> Result<Option<CommittedChange>, EditorError> {
    let lines = lines(text);
    let changes = selected_lines(&lines, selection)
        .filter_map(|index| {
            let line = &lines[index];
            let start = line.base(text);
            if add {
                Some(change(start..start, "    "))
            } else {
                let width = if text.as_bytes().get(start) == Some(&b'\t') {
                    1
                } else {
                    text[start..line.content_end]
                        .bytes()
                        .take_while(|&byte| byte == b' ')
                        .take(4)
                        .count()
                };
                (width > 0).then(|| change(start..start + width, ""))
            }
        })
        .collect();
    commit(
        editor,
        text,
        changes,
        selection,
        None,
        false,
        TransactionOrigin::Formatting,
    )
}

#[derive(Clone, Copy)]
struct FenceMarker {
    byte: u8,
    length: usize,
    content: usize,
}

fn fence_marker(text: &str, line: &Line) -> Option<FenceMarker> {
    let base = line.base(text);
    let at = skip_space(text, base, line.content_end);
    if at - base > 3 {
        return None;
    }
    let byte = *text.as_bytes().get(at)?;
    if !matches!(byte, b'`' | b'~') {
        return None;
    }
    let length = run_after(text, at, byte).min(line.content_end - at);
    (length >= 3).then_some(FenceMarker {
        byte,
        length,
        content: at + length,
    })
}

fn enclosing_fence(
    text: &str,
    lines: &[Line],
    first: usize,
    last: usize,
) -> Option<(usize, Option<usize>)> {
    let mut active: Option<(usize, FenceMarker)> = None;
    for (index, line) in lines.iter().enumerate() {
        let Some(marker) = fence_marker(text, line) else {
            continue;
        };
        if let Some((opening, start)) = active {
            if marker.byte == start.byte
                && marker.length >= start.length
                && text[marker.content..line.content_end].trim().is_empty()
            {
                if opening <= first && last <= index {
                    return Some((opening, Some(index)));
                }
                active = None;
            }
        } else if marker.byte != b'`' || !text[marker.content..line.content_end].contains('`') {
            active = Some((index, marker));
        }
    }
    active
        .filter(|(opening, _)| *opening <= first)
        .map(|(opening, _)| (opening, None))
}

fn newline<'a>(text: &'a str, lines: &[Line], index: usize) -> &'a str {
    let local = lines[index].ending(text);
    if !local.is_empty() {
        local
    } else {
        lines
            .iter()
            .map(|line| line.ending(text))
            .find(|ending| !ending.is_empty())
            .unwrap_or("\n")
    }
}

fn indented_code(text: &str, lines: &[Line], index: usize) -> bool {
    let current = prefix(text, &lines[index]);
    if !current.quotes.is_empty() {
        return false;
    }
    let columns = |value: &str| {
        value.bytes().fold(0_usize, |column, byte| {
            if byte == b'\t' {
                (column / 4 + 1) * 4
            } else {
                column + 1
            }
        })
    };
    let indent = columns(&text[current.base..current.indent_end]);
    if indent < 4 {
        return false;
    }
    for line in lines[..index].iter().rev() {
        if text[line.start..line.content_end].trim().is_empty() {
            continue;
        }
        let previous = prefix(text, line);
        let previous_indent = columns(&text[previous.base..previous.indent_end]);
        if previous_indent < indent {
            return previous.list.is_none();
        }
    }
    true
}

fn fence(
    editor: &mut Editor,
    text: &str,
    selection: Selection,
) -> Result<Option<CommittedChange>, EditorError> {
    let lines = lines(text);
    let target = selected_lines(&lines, selection);
    if let Some((open, close)) = enclosing_fence(text, &lines, target.start, target.end - 1) {
        let first = &lines[open];
        let mut changes = vec![change(first.base(text)..first.end, "")];
        if let Some(close) = close {
            let last = &lines[close];
            let start = if last.ending(text).is_empty() && close > open + 1 {
                lines[close - 1].content_end
            } else {
                last.start
            };
            changes.push(change(start..last.end, ""));
        }
        return commit(
            editor,
            text,
            changes,
            selection,
            None,
            false,
            TransactionOrigin::Formatting,
        );
    }
    let range = lines[target.start].base(text)..lines[target.end - 1].end;
    let value = &text[range.clone()];
    let delimiter = "`".repeat((max_backticks(value) + 1).max(3));
    let ending = newline(text, &lines, target.start);
    let open = format!("{delimiter}{ending}");
    let close = if value.ends_with(['\n', '\r']) {
        format!("{delimiter}{ending}")
    } else {
        format!("{ending}{delimiter}")
    };
    if range.is_empty() {
        return commit(
            editor,
            text,
            vec![change(range.clone(), format!("{open}{close}"))],
            selection,
            Some(Selection::caret(ByteOffset(range.start + open.len()))),
            false,
            TransactionOrigin::Formatting,
        );
    }
    let changes = vec![
        change(range.start..range.start, &open),
        change(range.end..range.end, close),
    ];
    let explicit = if selection.is_collapsed() {
        let map = ChangeMap {
            before_bytes: text.len(),
            changes: changes.clone(),
        };
        Some(Selection::caret(map.map_offset(
            selection.head,
            if selection.head.0 == range.end {
                Affinity::Before
            } else {
                Affinity::After
            },
        )?))
    } else {
        Some(mapped_selection(text, &changes, selection, true)?)
    };
    commit(
        editor,
        text,
        changes,
        selection,
        explicit,
        true,
        TransactionOrigin::Formatting,
    )
}

fn enter(
    editor: &mut Editor,
    text: &str,
    selection: Selection,
) -> Result<Option<CommittedChange>, EditorError> {
    let lines = lines(text);
    let index = line_at(&lines, selection.start().0);
    let line = &lines[index];
    let ending = newline(text, &lines, index);
    let prefix = prefix(text, line);
    let mut continuation = String::new();
    let in_fence = enclosing_fence(text, &lines, index, index)
        .is_some_and(|(open, close)| open < index && close != Some(index))
        || indented_code(text, &lines, index);
    if selection.is_collapsed() && selection.head.0 >= prefix.content && !in_fence {
        let tail = &text[prefix.content..line.content_end];
        let task = prefix.list.is_some()
            && (tail.starts_with("[ ] ") || tail.starts_with("[x] ") || tail.starts_with("[X] "));
        let empty = if task {
            tail[4..].trim().is_empty()
        } else {
            tail.trim().is_empty()
        };
        if let Some(marker) = &prefix.list {
            if empty {
                let start = if prefix.quotes.is_empty() {
                    prefix.base
                } else {
                    marker.range.start
                };
                return commit(
                    editor,
                    text,
                    vec![change(start..line.content_end, "")],
                    selection,
                    Some(Selection::caret(ByteOffset(start))),
                    false,
                    TransactionOrigin::Formatting,
                );
            }
            continuation.push_str(&text[prefix.base..marker.range.start]);
            if marker.kind == ListKind::Ordered {
                let next = marker
                    .number
                    .checked_add(1)
                    .filter(|number| *number <= 999_999_999)
                    .unwrap_or(marker.number);
                continuation.push_str(&format!(
                    "{next:0width$}{}",
                    marker.delimiter as char,
                    width = marker.digits
                ));
            } else {
                continuation.push(marker.delimiter as char);
            }
            continuation.push_str(if marker.padding.is_empty() {
                " "
            } else {
                &marker.padding
            });
            if task {
                continuation.push_str("[ ] ");
            }
        } else if !prefix.quotes.is_empty() {
            if empty {
                let start = prefix.quotes.last().expect("quote exists").start;
                return commit(
                    editor,
                    text,
                    vec![change(start..line.content_end, "")],
                    selection,
                    Some(Selection::caret(ByteOffset(start))),
                    false,
                    TransactionOrigin::Formatting,
                );
            }
            continuation.push_str(&text[prefix.base..prefix.marker_start]);
        } else {
            continuation.push_str(&text[prefix.base..prefix.indent_end]);
        }
    } else if selection.is_collapsed() && selection.head.0 >= prefix.indent_end {
        continuation.push_str(&text[prefix.base..prefix.indent_end]);
    }
    let inserted = format!("{ending}{continuation}");
    let at = selection.start().0;
    let after = Selection::caret(ByteOffset(at + inserted.len()));
    commit(
        editor,
        text,
        vec![change(at..selection.end().0, inserted)],
        selection,
        Some(after),
        false,
        TransactionOrigin::Typing,
    )
}

fn backspace(
    editor: &mut Editor,
    text: &str,
    selection: Selection,
) -> Result<Option<CommittedChange>, EditorError> {
    if !selection.is_collapsed() {
        return editor.delete_backward();
    }
    let lines = lines(text);
    let index = line_at(&lines, selection.head.0);
    let line = &lines[index];
    let prefix = prefix(text, line);
    let at = selection.head.0;
    let in_fence = enclosing_fence(text, &lines, index, index)
        .is_some_and(|(open, close)| open < index && close != Some(index))
        || indented_code(text, &lines, index);
    let range = if !in_fence && at == prefix.content {
        if let Some(heading) = prefix.heading {
            Some(heading)
        } else if let Some(list) = prefix.list {
            Some(list.range)
        } else {
            prefix.quotes.last().cloned()
        }
    } else {
        None
    };
    if let Some(range) = range {
        let after = Selection::caret(ByteOffset(range.start));
        return commit(
            editor,
            text,
            vec![change(range, "")],
            selection,
            Some(after),
            false,
            TransactionOrigin::Delete,
        );
    }
    if at == prefix.indent_end && prefix.indent_end > prefix.base {
        let width = if text.as_bytes()[prefix.base] == b'\t' {
            1
        } else {
            (prefix.indent_end - prefix.base).min(4)
        };
        return commit(
            editor,
            text,
            vec![change(prefix.base..prefix.base + width, "")],
            selection,
            Some(Selection::caret(ByteOffset(at - width))),
            false,
            TransactionOrigin::Delete,
        );
    }
    editor.delete_backward()
}

// Deliberate line-local commands; source spelling outside their edits is retained.
fn strike(
    editor: &mut Editor,
    text: &str,
    selection: Selection,
) -> Result<Option<CommittedChange>, EditorError> {
    if selection.is_collapsed() {
        let at = selection.head.0;
        return commit(
            editor,
            text,
            vec![change(at..at, "~~~~")],
            selection,
            Some(Selection::caret(ByteOffset(at + 2))),
            true,
            TransactionOrigin::Formatting,
        );
    }
    let ranges: Vec<_> = lines(text)
        .iter()
        .filter_map(|line| {
            let start = line.start.max(selection.start().0);
            let end = line.content_end.min(selection.end().0);
            (start < end)
                .then(|| trimmed_range(text, start..end))
                .flatten()
        })
        .collect();
    let envelopes: Vec<_> = ranges
        .iter()
        .map(|range| {
            if range.len() >= 4
                && text[range.clone()].starts_with("~~")
                && text[range.clone()].ends_with("~~")
                && !escaped(text, range.start)
                && !escaped(text, range.end - 2)
            {
                Some((range.start..range.start + 2, range.end - 2..range.end))
            } else if range.start >= 2
                && text[..range.start].ends_with("~~")
                && text[range.end..].starts_with("~~")
                && !escaped(text, range.start - 2)
                && !escaped(text, range.end)
            {
                Some((range.start - 2..range.start, range.end..range.end + 2))
            } else {
                None
            }
        })
        .collect();
    let remove = !envelopes.is_empty() && envelopes.iter().all(Option::is_some);
    let mut changes = Vec::new();
    for (range, envelope) in ranges.into_iter().zip(envelopes) {
        if remove {
            let (open, close) = envelope.expect("all selected lines are marked");
            changes.extend([change(open, ""), change(close, "")]);
        } else if envelope.is_none() {
            changes.extend([
                change(range.start..range.start, "~~"),
                change(range.end..range.end, "~~"),
            ]);
        }
    }
    commit(
        editor,
        text,
        changes,
        selection,
        None,
        true,
        TransactionOrigin::Formatting,
    )
}

fn tasks(
    editor: &mut Editor,
    text: &str,
    selection: Selection,
    check: bool,
) -> Result<Option<CommittedChange>, EditorError> {
    let all = lines(text);
    let first = line_at(&all, selection.start().0);
    let last = line_at(
        &all,
        if selection.is_collapsed() {
            selection.end().0
        } else {
            selection.end().0.saturating_sub(1)
        },
    );
    let remove_tasks = !check
        && all
            .iter()
            .enumerate()
            .take(last + 1)
            .skip(first)
            .filter(|(index, _)| {
                enclosing_fence(text, &all, *index, *index).is_none()
                    && !indented_code(text, &all, *index)
            })
            .all(|(_, line)| {
                let prefix = prefix(text, line);
                let tail = &text[prefix.content..line.content_end];
                prefix.list.is_some()
                    && (tail.starts_with("[ ]")
                        || tail.starts_with("[x]")
                        || tail.starts_with("[X]"))
                    && (tail.len() == 3
                        || tail.as_bytes().get(3).is_some_and(u8::is_ascii_whitespace))
            });
    let mut changes = Vec::new();
    for (index, line) in all.iter().enumerate().take(last + 1).skip(first) {
        if enclosing_fence(text, &all, index, index).is_some() || indented_code(text, &all, index) {
            continue;
        }
        let prefix = prefix(text, line);
        let at = prefix.content;
        let tail = &text[at..line.content_end];
        let marker = prefix.list.is_some()
            && (tail.starts_with("[ ]") || tail.starts_with("[x]") || tail.starts_with("[X]"))
            && (tail.len() == 3 || tail.as_bytes().get(3).is_some_and(u8::is_ascii_whitespace));
        if marker {
            if check {
                changes.push(change(
                    at + 1..at + 2,
                    if tail.as_bytes()[1] == b' ' { "x" } else { " " },
                ));
            } else if remove_tasks {
                changes.push(change(
                    at..at + 3 + usize::from(tail.as_bytes().get(3) == Some(&b' ')),
                    "",
                ));
            }
        } else if !check {
            let at = if prefix.list.is_some() {
                at
            } else {
                prefix.marker_start
            };
            changes.push(change(
                at..at,
                if prefix.list.is_some() {
                    "[ ] "
                } else {
                    "- [ ] "
                },
            ));
        }
    }
    commit(
        editor,
        text,
        changes,
        selection,
        None,
        false,
        TransactionOrigin::Formatting,
    )
}
