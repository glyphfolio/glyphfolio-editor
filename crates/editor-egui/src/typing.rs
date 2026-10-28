//! Presentation-only typing marks; no empty Markdown delimiters are persisted.
use glyphfolio_editor_core::{DocumentId, EditCommand, Editor, Revision, Selection};
#[derive(Clone)]
pub(crate) struct PendingMarks {
    pub document: DocumentId,
    pub revision: Revision,
    pub selection: Selection,
    pub marks: Vec<EditCommand>,
}
impl PendingMarks {
    pub fn new(editor: &Editor, marks: Vec<EditCommand>) -> Self {
        let snapshot = editor.snapshot();
        Self {
            document: snapshot.document_id(),
            revision: snapshot.revision(),
            selection: editor.selection(),
            marks,
        }
    }
    pub fn valid(&self, editor: &Editor) -> bool {
        let snapshot = editor.snapshot();
        self.document == snapshot.document_id()
            && self.revision == snapshot.revision()
            && self.selection == editor.selection()
            && editor.selection().is_collapsed()
    }
    pub fn materialize(&self, text: &str) -> (String, usize, bool) {
        let body = text.trim_matches(char::is_whitespace);
        if body.is_empty() {
            return (text.into(), text.len(), true);
        }
        let leading = text.len() - text.trim_start_matches(char::is_whitespace).len();
        let trailing = text.len() - text.trim_end_matches(char::is_whitespace).len();
        let mut marks = self.marks.clone();
        marks.sort_by_key(|mark| match mark {
            EditCommand::ToggleStrong => 0,
            EditCommand::ToggleEmphasis => 1,
            EditCommand::ToggleStrikethrough => 2,
            _ => 3,
        });
        let mut prefix = String::new();
        let mut suffix = String::new();
        for mark in marks {
            let (open, close) = match mark {
                EditCommand::ToggleStrong => ("**".into(), "**".into()),
                EditCommand::ToggleEmphasis => ("*".into(), "*".into()),
                EditCommand::ToggleStrikethrough => ("~~".into(), "~~".into()),
                EditCommand::ToggleInlineCode => {
                    let count = body.split(|ch| ch != '`').map(str::len).max().unwrap_or(0) + 1;
                    let ticks = "`".repeat(count);
                    let pad = if body.starts_with('`') || body.ends_with('`') {
                        " "
                    } else {
                        ""
                    };
                    (format!("{ticks}{pad}"), format!("{pad}{ticks}"))
                }
                _ => continue,
            };
            prefix.push_str(&open);
            suffix = format!("{close}{suffix}");
        }
        let source = format!(
            "{}{}{}{}{}",
            &text[..leading],
            prefix,
            body,
            suffix,
            &text[text.len() - trailing..]
        );
        let caret = if trailing > 0 {
            source.len()
        } else {
            leading + prefix.len() + body.len()
        };
        (source, caret, trailing > 0)
    }
}
pub(crate) fn inline(command: EditCommand) -> bool {
    matches!(
        command,
        EditCommand::ToggleStrong
            | EditCommand::ToggleEmphasis
            | EditCommand::ToggleStrikethrough
            | EditCommand::ToggleInlineCode
    )
}
