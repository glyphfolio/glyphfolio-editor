//! Host-independent link dialog; document edits remain revision-checked transactions.
use glyphfolio_editor_core::{
    Affinity, ByteOffset, DocumentId, Editor, Revision, Selection, Transaction, TransactionOrigin,
};
use glyphfolio_editor_markdown::{MarkdownDialect, MarkdownParser, StyleKind};
pub(crate) struct LinkDraft {
    document: DocumentId,
    revision: Revision,
    selection: Selection,
    original: (String, String, bool),
    existing: bool,
    title: String,
    pub label: String,
    pub destination: String,
    pub wiki: bool,
}
impl LinkDraft {
    pub fn capture(editor: &Editor) -> Self {
        let snapshot = editor.snapshot();
        let source = snapshot.text();
        let mut selected = editor.selection();
        let parsed = MarkdownParser::with_dialect(MarkdownDialect::GfmWiki).parse(&snapshot);
        let link = parsed.styles().iter().find(|span| {
            matches!(span.kind, StyleKind::Link { .. })
                && span.source_range.start <= selected.start()
                && selected.end() <= span.source_range.end
        });
        let title = link
            .and_then(|span| {
                if let StyleKind::Link { title, .. } = &span.kind {
                    Some(title.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();
        let (label, destination, wiki) = if let Some(span) = link {
            selected = Selection {
                anchor: span.source_range.start,
                head: span.source_range.end,
                affinity: Affinity::After,
            };
            let StyleKind::Link { destination, .. } = &span.kind else {
                unreachable!()
            };
            let raw = &source[span.source_range.start.0..span.source_range.end.0];
            let wiki = destination.starts_with("wiki:");
            let label = if wiki {
                raw.trim_start_matches("[[")
                    .trim_end_matches("]]")
                    .split_once('|')
                    .map_or_else(|| destination[5..].to_owned(), |(_, label)| label.into())
            } else {
                raw_label(raw).unwrap_or_default().to_owned()
            };
            (
                label,
                if wiki {
                    destination[5..].to_owned()
                } else {
                    destination.clone()
                },
                wiki,
            )
        } else {
            (
                snapshot.slice(selected.range()).unwrap_or_default(),
                String::new(),
                false,
            )
        };
        Self {
            document: snapshot.document_id(),
            revision: snapshot.revision(),
            selection: selected,
            original: (label.clone(), destination.clone(), wiki),
            existing: link.is_some(),
            title,
            label,
            destination,
            wiki,
        }
    }
    pub fn apply(&self, editor: &mut Editor, remove: bool) -> Result<(), String> {
        let snapshot = editor.snapshot();
        if snapshot.document_id() != self.document || snapshot.revision() != self.revision {
            return Err(
                "Document changed while the link dialog was open; reopen the dialog".into(),
            );
        }
        if !remove
            && self.existing
            && self.original == (self.label.clone(), self.destination.clone(), self.wiki)
        {
            return Ok(());
        }
        let label = if self.existing && !self.original.2 && self.label == self.original.0 {
            self.label.clone()
        } else {
            self.label
                .replace('\\', "\\\\")
                .replace('[', "\\[")
                .replace(']', "\\]")
                .replace(['\r', '\n'], " ")
        };
        let value = if remove {
            label
        } else if self.wiki {
            if self.destination.trim().is_empty()
                || self.destination.contains(['[', ']', '|', '\r', '\n'])
                || self.label.contains(['[', ']', '|', '\r', '\n'])
            {
                return Err("Wiki links need a note name and a valid label".into());
            }
            if self.label == self.destination {
                format!("[[{}]]", self.destination.trim())
            } else {
                format!("[[{}|{}]]", self.destination.trim(), self.label)
            }
        } else {
            if !glyphfolio_editor_markdown::safe_link(&self.destination) {
                return Err("Enter a safe URL or relative note reference".into());
            }
            let title = if self.title.is_empty() || self.original.2 {
                String::new()
            } else {
                format!(
                    " \"{}\"",
                    self.title.replace('\\', "\\\\").replace('"', "\\\"")
                )
            };
            format!(
                "[{label}](<{}>{title})",
                self.destination
                    .trim()
                    .replace('<', "%3C")
                    .replace('>', "%3E")
            )
        };
        let at = self.selection.start().0;
        editor
            .apply(
                Transaction::new(&snapshot)
                    .replace(self.selection.range(), value.clone())
                    .with_selection(Selection::caret(ByteOffset(at + value.len())))
                    .with_origin(TransactionOrigin::Formatting),
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

pub(crate) fn raw_label(source: &str) -> Option<&str> {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn link_dialog_transactions_reject_stale_state_and_undo_exactly() {
        let mut editor = Editor::new("日本");
        editor
            .set_selection(Selection {
                anchor: ByteOffset(0),
                head: ByteOffset(6),
                ..Selection::default()
            })
            .unwrap();
        let mut draft = LinkDraft::capture(&editor);
        draft.destination = "https://example.org".into();
        draft.apply(&mut editor, false).unwrap();
        assert_eq!(editor.snapshot().text(), "[日本](<https://example.org>)");
        editor.undo().unwrap();
        assert_eq!(editor.snapshot().text(), "日本");
        assert!(draft.apply(&mut editor, false).is_err());
        let mut draft = LinkDraft::capture(&editor);
        draft.wiki = true;
        draft.destination = "Note".into();
        draft.apply(&mut editor, false).unwrap();
        assert_eq!(editor.snapshot().text(), "[[Note|日本]]");
    }
}

#[cfg(test)]
mod fidelity_tests {
    use super::*;
    #[test]
    fn unchanged_link_confirmation_keeps_source_and_history_exact() {
        for source in [
            "[a\\]b](https://example.org 'a title')",
            "[[Note|label]]",
            "[**bold**](<https://example.org>)",
        ] {
            let mut editor = Editor::new(source);
            editor
                .set_selection(Selection::caret(ByteOffset(2)))
                .unwrap();
            let before = editor.snapshot();
            let draft = LinkDraft::capture(&editor);
            draft.apply(&mut editor, false).unwrap();
            assert_eq!(editor.snapshot().text(), source);
            assert_eq!(editor.snapshot().revision(), before.revision());
            assert!(!editor.can_undo());
        }
    }
    #[test]
    fn changing_destination_keeps_escaped_label_and_title_semantics() {
        let source = "[a\\]b](https://old.example 'title')";
        let mut editor = Editor::new(source);
        editor
            .set_selection(Selection::caret(ByteOffset(2)))
            .unwrap();
        let mut draft = LinkDraft::capture(&editor);
        draft.destination = "https://new.example".into();
        draft.apply(&mut editor, false).unwrap();
        assert_eq!(
            editor.snapshot().text(),
            "[a\\]b](<https://new.example> \"title\")"
        );
        editor.undo().unwrap();
        assert_eq!(editor.snapshot().text(), source);
    }
}
