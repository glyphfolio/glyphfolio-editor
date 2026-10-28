//! Original source-preserving GFM table operations. No AST serialization.
use crate::{
    ByteOffset, CommittedChange, Editor, EditorError, Selection, Transaction, TransactionOrigin,
};
use std::ops::Range;

#[derive(Clone, Debug)]
pub struct TableRow {
    pub range: Range<ByteOffset>,
    /// Trimmed source content, excluding pipes and cell padding.
    pub cells: Vec<Range<ByteOffset>>,
}
#[derive(Clone, Debug)]
pub struct MarkdownTable {
    pub range: Range<ByteOffset>,
    /// Header followed by body rows; the separator is stored separately.
    pub rows: Vec<TableRow>,
    pub separator: TableRow,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColumnAlignment {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableCommand {
    Insert,
    AddRow,
    DeleteRow,
    AddColumn,
    DeleteColumn,
    NextCell,
    PreviousCell,
    NextRow,
    PreviousRow,
    ClearCell,
    DeleteTable,
    Exit,
    Align(ColumnAlignment),
}

fn row(text: &str, start: usize, end: usize) -> Option<TableRow> {
    let line = &text[start..end];
    let content = line.trim_end_matches(['\r', '\n']);
    if !content.contains('|') {
        return None;
    }
    let mut pipes = Vec::new();
    let mut slash = 0;
    for (at, byte) in content.bytes().enumerate() {
        if byte == b'|' && slash % 2 == 0 {
            pipes.push(at);
        }
        slash = if byte == b'\\' { slash + 1 } else { 0 };
    }
    if pipes.is_empty() {
        return None;
    }
    let bom = if start == 0 && content.starts_with('\u{feff}') {
        3
    } else {
        0
    };
    let left = bom + content[bom..].len() - content[bom..].trim_start().len();
    let right = content.trim_end().len();
    let mut boundaries = vec![left];
    boundaries.extend(pipes.iter().map(|at| at + 1));
    let mut ends = pipes.clone();
    ends.push(right);
    let mut cells = Vec::new();
    for (a, b) in boundaries.into_iter().zip(ends) {
        if a > b
            || (a == left && b == left)
            || (a == right && content.as_bytes().get(a.saturating_sub(1)) == Some(&b'|'))
        {
            continue;
        }
        let value = &content[a..b];
        let begin = a + value.len() - value.trim_start().len();
        let finish = a + value.trim_end().len();
        cells.push(ByteOffset(start + begin)..ByteOffset(start + finish.max(begin)));
    }
    (!cells.is_empty()).then_some(TableRow {
        range: ByteOffset(start)..ByteOffset(end),
        cells,
    })
}
impl MarkdownTable {
    /// Locate a top-level pipe table. Fenced code and indented lines are excluded.
    pub fn at(text: &str, offset: ByteOffset) -> Option<Self> {
        let mut lines = Vec::new();
        let mut at = 0;
        for line in text.split_inclusive('\n') {
            lines.push((at, at + line.len()));
            at += line.len();
        }
        let mut index = 0;
        let mut fence: Option<(u8, usize)> = None;
        while index + 1 < lines.len() {
            let (start, end) = lines[index];
            let value = text[start..end].trim_end_matches(['\r', '\n']);
            let value = if start == 0 {
                value.strip_prefix('\u{feff}').unwrap_or(value)
            } else {
                value
            };
            let trimmed = value.trim_start();
            let indent = value.len() - trimmed.len();
            let byte = trimmed.as_bytes().first().copied().unwrap_or(0);
            let run = trimmed.bytes().take_while(|b| *b == byte).count();
            if indent <= 3 && matches!(byte, b'`' | b'~') && run >= 3 {
                if let Some((old, count)) = fence {
                    if byte == old && run >= count && trimmed[run..].trim().is_empty() {
                        fence = None;
                    }
                } else {
                    fence = Some((byte, run));
                }
                index += 1;
                continue;
            }
            if fence.is_some() || indent >= 4 {
                index += 1;
                continue;
            }
            let Some(header) = row(text, start, end) else {
                index += 1;
                continue;
            };
            let (a, b) = lines[index + 1];
            let Some(separator) = row(text, a, b) else {
                index += 1;
                continue;
            };
            if header.cells.len() != separator.cells.len()
                || !separator.cells.iter().all(|cell| {
                    let value = text[cell.start.0..cell.end.0].trim_matches(':');
                    !value.is_empty() && value.bytes().all(|b| b == b'-')
                })
            {
                index += 1;
                continue;
            }
            let mut rows = vec![header];
            let begin = start;
            let mut finish = b;
            index += 2;
            while index < lines.len() {
                let (a, b) = lines[index];
                if text[a..b].trim().is_empty() {
                    break;
                }
                let Some(next) = row(text, a, b) else {
                    break;
                };
                finish = b;
                rows.push(next);
                index += 1;
            }
            if begin <= offset.0
                && (offset.0 < finish
                    || offset.0 == finish && finish == text.len() && !text.ends_with(['\n', '\r']))
            {
                return Some(Self {
                    range: ByteOffset(begin)..ByteOffset(finish),
                    rows,
                    separator,
                });
            }
        }
        None
    }
    pub fn alignments(&self, text: &str) -> Vec<ColumnAlignment> {
        self.separator
            .cells
            .iter()
            .map(|cell| {
                let value = text[cell.start.0..cell.end.0].trim();
                if value.ends_with(':') {
                    if value.starts_with(':') {
                        ColumnAlignment::Center
                    } else {
                        ColumnAlignment::Right
                    }
                } else {
                    ColumnAlignment::Left
                }
            })
            .collect()
    }
    pub fn cell_at(&self, offset: ByteOffset) -> (usize, usize) {
        let row = self
            .rows
            .iter()
            .position(|row| offset < row.range.end)
            .unwrap_or(self.rows.len() - 1);
        let column = self.rows[row]
            .cells
            .iter()
            .position(|cell| offset <= cell.end)
            .unwrap_or(self.rows[row].cells.len() - 1);
        (row, column)
    }
}
impl Editor {
    /// Execute a table operation atomically against the current source snapshot.
    /// Unsupported/ragged structures are left intact rather than normalized.
    pub fn edit_table(
        &mut self,
        command: TableCommand,
    ) -> Result<Option<CommittedChange>, EditorError> {
        let snapshot = self.snapshot();
        let text = snapshot.text();
        let selected = self.selection();
        let table = MarkdownTable::at(&text, selected.head);
        let ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
        if command == TableCommand::Insert {
            let at = selected.start().0;
            let prefix = if at > 0 && !text[..at].ends_with('\n') {
                ending
            } else {
                ""
            };
            let value = format!(
                "{prefix}| Column 1 | Column 2 |{ending}| --- | --- |{ending}|  |  |{ending}"
            );
            return self
                .apply(
                    Transaction::new(&snapshot)
                        .replace(selected.range(), value)
                        .with_selection(Selection::caret(ByteOffset(at + prefix.len() + 2)))
                        .with_origin(TransactionOrigin::Formatting),
                )
                .map(Some);
        }
        let Some(table) = table else {
            return Ok(None);
        };
        let (r, c) = table.cell_at(selected.head);
        let width = table.separator.cells.len();
        let mut operation = command;
        let mut target_column = c;
        if matches!(
            command,
            TableCommand::NextCell
                | TableCommand::PreviousCell
                | TableCommand::NextRow
                | TableCommand::PreviousRow
        ) {
            let target = match command {
                TableCommand::NextCell if c + 1 < table.rows[r].cells.len() => Some((r, c + 1)),
                TableCommand::NextCell if r + 1 < table.rows.len() => Some((r + 1, 0)),
                TableCommand::PreviousCell if c > 0 => Some((r, c - 1)),
                TableCommand::PreviousCell if r > 0 => {
                    Some((r - 1, table.rows[r - 1].cells.len() - 1))
                }
                TableCommand::PreviousCell => Some((r, c)),
                TableCommand::NextRow if r + 1 < table.rows.len() => {
                    Some((r + 1, c.min(table.rows[r + 1].cells.len() - 1)))
                }
                TableCommand::PreviousRow => {
                    let next = r.saturating_sub(1);
                    Some((next, c.min(table.rows[next].cells.len() - 1)))
                }
                _ => None,
            };
            if let Some((r, c)) = target {
                let cell = &table.rows[r].cells[c];
                self.set_selection(Selection {
                    anchor: cell.start,
                    head: cell.end,
                    ..Selection::default()
                })?;
                return Ok(None);
            }
            operation = TableCommand::AddRow;
            target_column = if command == TableCommand::NextCell {
                0
            } else {
                c
            };
        }
        let mut tx = Transaction::new(&snapshot).with_origin(TransactionOrigin::Formatting);
        match operation {
            TableCommand::AddRow => {
                let at = if r == 0 {
                    table.separator.range.end
                } else {
                    table.rows[r].range.end
                };
                let prefix = if text[..at.0].ends_with('\n') {
                    ""
                } else {
                    ending
                };
                let value = format!("{prefix}| {} |{ending}", vec![" "; width].join(" | "));
                // The separator always follows the header.
                let caret = row(&value, prefix.len(), value.len())
                    .and_then(|row| {
                        row.cells
                            .get(target_column.min(width - 1))
                            .map(|cell| cell.start.0)
                    })
                    .unwrap_or(prefix.len() + 2);
                tx = tx
                    .replace(at..at, value)
                    .with_selection(Selection::caret(ByteOffset(at.0 + caret)));
            }
            TableCommand::Exit => {
                let at = table.range.end;
                if text[at.0..].starts_with(ending) {
                    self.set_selection(Selection::caret(ByteOffset(at.0 + ending.len())))?;
                    return Ok(None);
                }
                let gap = if text[..at.0].ends_with('\n') {
                    ending.to_owned()
                } else {
                    ending.repeat(2)
                };
                tx = tx
                    .replace(at..at, gap.clone())
                    .with_selection(Selection::caret(ByteOffset(at.0 + gap.len())));
            }
            TableCommand::DeleteTable => {
                let start = if table.range.start.0 == 0 && text.starts_with('\u{feff}') {
                    ByteOffset(3)
                } else {
                    table.range.start
                };
                tx = tx
                    .replace(start..table.range.end, "")
                    .with_selection(Selection::caret(start));
            }
            TableCommand::ClearCell => {
                let cell = table.rows[r].cells[c].clone();
                if cell.is_empty() {
                    return Ok(None);
                }
                tx = tx
                    .replace(cell.clone(), "")
                    .with_selection(Selection::caret(cell.start));
            }
            TableCommand::Align(alignment) => {
                if c >= width || table.rows.iter().any(|row| row.cells.len() != width) {
                    return Ok(None);
                }
                let cell = table.separator.cells[c].clone();
                let value = match alignment {
                    ColumnAlignment::Left => ":---",
                    ColumnAlignment::Center => ":---:",
                    ColumnAlignment::Right => "---:",
                };
                if text[cell.start.0..cell.end.0] == *value {
                    return Ok(None);
                }
                tx = tx
                    .replace(cell, value)
                    .with_grapheme_selection(crate::Affinity::After);
            }
            TableCommand::DeleteRow => {
                if table.rows.len() == 1 {
                    return self.edit_table(TableCommand::DeleteTable);
                }
                if r == 0 {
                    let header = &table.rows[0];
                    let promoted = &table.rows[1];
                    if promoted.cells.len() != width {
                        return Ok(None);
                    }
                    let bom = if header.range.start.0 == 0 && text.starts_with('\u{feff}') {
                        "\u{feff}"
                    } else {
                        ""
                    };
                    let raw = &text[promoted.range.start.0..promoted.range.end.0];
                    let value = if raw.ends_with('\n') {
                        format!("{bom}{raw}")
                    } else {
                        format!("{bom}{raw}{ending}")
                    };
                    let caret = header.range.start.0
                        + bom.len()
                        + promoted.cells[c.min(promoted.cells.len() - 1)].start.0
                        - promoted.range.start.0;
                    tx = tx
                        .replace(header.range.clone(), value)
                        .replace(promoted.range.clone(), "")
                        .with_selection(Selection::caret(ByteOffset(caret)));
                } else {
                    let range = table.rows[r].range.clone();
                    tx = tx.replace(range, "").with_selection(Selection::caret(
                        table.rows[r - 1].cells[c.min(table.rows[r - 1].cells.len() - 1)].start,
                    ));
                }
            }
            TableCommand::AddColumn | TableCommand::DeleteColumn => {
                if table.rows.iter().any(|row| row.cells.len() != width) {
                    return Ok(None);
                }
                if operation == TableCommand::DeleteColumn && width == 1 {
                    return self.edit_table(TableCommand::DeleteTable);
                }
                for row in table.rows.iter().chain(std::iter::once(&table.separator)) {
                    let cell = &row.cells[c];
                    if operation == TableCommand::AddColumn {
                        let marker = if row.range == table.separator.range {
                            " | ---"
                        } else {
                            " | "
                        };
                        tx = tx.replace(cell.end..cell.end, marker);
                    } else {
                        let range = if c + 1 < width {
                            cell.start..row.cells[c + 1].start
                        } else {
                            row.cells[c - 1].end..cell.end
                        };
                        tx = tx.replace(range, "");
                    }
                }
                tx = tx.with_grapheme_selection(crate::Affinity::After);
            }
            TableCommand::NextCell
            | TableCommand::PreviousCell
            | TableCommand::NextRow
            | TableCommand::PreviousRow => unreachable!("navigation handled before editing"),
            TableCommand::Insert => unreachable!(),
        }
        self.apply(tx).map(Some)
    }
}
