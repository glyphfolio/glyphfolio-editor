//! Host-owned synchronous clipboard acknowledgement, independent of toolkit events.

use glyphfolio_editor_core::{Editor, Selection, Snapshot, Transaction, TransactionOrigin};

/// A successful return acknowledges publication to the platform clipboard.
/// Providers must not return success merely because they queued a future write.
pub trait ClipboardService {
    fn write_text(&mut self, text: &str) -> Result<(), String>;
    fn read_text(&mut self) -> Result<String, String>;
    fn read_files(&mut self) -> Result<Vec<std::path::PathBuf>, String> {
        Ok(Vec::new())
    }
    fn read_image(&mut self) -> Result<Option<crate::assets::AssetImage>, String> {
        Ok(None)
    }
    /// Optional HTML representation of the same clipboard publication.
    fn read_html(&mut self) -> Result<Option<String>, String> {
        Ok(None)
    }
}

/// Native text and optional HTML transport. Binary assets remain host-owned.
pub struct SystemClipboard(arboard::Clipboard);

impl SystemClipboard {
    pub fn new() -> Result<Self, String> {
        arboard::Clipboard::new()
            .map(Self)
            .map_err(|error| error.to_string())
    }
}

impl ClipboardService for SystemClipboard {
    fn read_image(&mut self) -> Result<Option<crate::assets::AssetImage>, String> {
        match self.0.get_image() {
            Ok(image) => {
                let image = crate::assets::AssetImage {
                    size: [image.width, image.height],
                    rgba: image.bytes.into_owned(),
                };
                image.validate()?;
                Ok(Some(image))
            }
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn read_files(&mut self) -> Result<Vec<std::path::PathBuf>, String> {
        match self.0.get().file_list() {
            Ok(files) => Ok(files),
            Err(arboard::Error::ContentNotAvailable) => Ok(Vec::new()),
            Err(error) => Err(error.to_string()),
        }
    }
    fn read_html(&mut self) -> Result<Option<String>, String> {
        match self.0.get().html() {
            Ok(html) => Ok(Some(html)),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }
    fn write_text(&mut self, text: &str) -> Result<(), String> {
        self.0.set_text(text).map_err(|error| error.to_string())
    }
    fn read_text(&mut self) -> Result<String, String> {
        self.0.get_text().map_err(|error| error.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardResult {
    EmptySelection,
    Copied,
    Cut,
}

/// Capture the intended source selection before an acknowledged cut.
/// All text stays in canonical Markdown coordinates, never provisional IME text.
pub struct ClipboardRequest {
    snapshot: Snapshot,
    selection: Selection,
}

impl ClipboardRequest {
    pub fn capture(editor: &Editor) -> Self {
        Self {
            snapshot: editor.snapshot(),
            selection: editor.selection(),
        }
    }

    fn validate(&self, editor: &Editor) -> Result<(), String> {
        let current = editor.snapshot();
        if current.document_id() != self.snapshot.document_id()
            || current.revision() != self.snapshot.revision()
            || editor.selection() != self.selection
        {
            return Err("Clipboard operation belongs to an older document or selection".into());
        }
        Ok(())
    }

    pub fn publish(
        self,
        editor: &mut Editor,
        clipboard: &mut dyn ClipboardService,
        cut: bool,
    ) -> Result<ClipboardResult, String> {
        self.validate(editor)?;
        if self.selection.is_collapsed() {
            return Ok(ClipboardResult::EmptySelection);
        }
        let source = self
            .snapshot
            .slice(self.selection.range())
            .map_err(|error| error.to_string())?;
        clipboard.write_text(&source)?;
        if !cut {
            return Ok(ClipboardResult::Copied);
        }
        // Publication has succeeded. Recheck identity and submit the captured
        // transaction rather than deleting whatever a later selection points at.
        self.validate(editor)?;
        editor
            .apply(
                Transaction::new(&self.snapshot)
                    .replace(self.selection.range(), "")
                    .with_selection(Selection::caret(self.selection.start()))
                    .with_origin(TransactionOrigin::Delete),
            )
            .map_err(|error| error.to_string())?;
        Ok(ClipboardResult::Cut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glyphfolio_editor_core::ByteOffset;

    #[derive(Default)]
    struct MemoryClipboard {
        text: String,
        fail: bool,
        writes: usize,
    }
    impl ClipboardService for MemoryClipboard {
        fn write_text(&mut self, text: &str) -> Result<(), String> {
            self.writes += 1;
            if self.fail {
                Err("clipboard occupied".into())
            } else {
                self.text = text.into();
                Ok(())
            }
        }
        fn read_text(&mut self) -> Result<String, String> {
            Ok(self.text.clone())
        }
    }

    #[test]
    fn cut_requires_successful_publication_and_is_one_undo_step() {
        let mut editor = Editor::new("**日本**\r\n");
        editor
            .set_selection(Selection {
                anchor: ByteOffset(8),
                head: ByteOffset(2),
                ..Selection::default()
            })
            .unwrap();
        let initial = editor.snapshot();
        let mut clipboard = MemoryClipboard {
            fail: true,
            ..Default::default()
        };
        assert!(
            ClipboardRequest::capture(&editor)
                .publish(&mut editor, &mut clipboard, true)
                .is_err()
        );
        assert_eq!(editor.snapshot().revision(), initial.revision());
        assert_eq!(editor.snapshot().text(), initial.text());
        assert!(!editor.can_undo());
        clipboard.fail = false;
        assert_eq!(
            ClipboardRequest::capture(&editor)
                .publish(&mut editor, &mut clipboard, true)
                .unwrap(),
            ClipboardResult::Cut
        );
        assert_eq!(clipboard.text, "日本");
        assert_eq!(editor.snapshot().text(), "****\r\n");
        editor.undo().unwrap().unwrap();
        assert_eq!(editor.snapshot().text(), initial.text());
    }

    #[test]
    fn stale_selection_or_session_is_rejected_before_clipboard_mutation() {
        let mut editor = Editor::new("abc");
        editor
            .set_selection(Selection {
                anchor: ByteOffset(0),
                head: ByteOffset(2),
                ..Selection::default()
            })
            .unwrap();
        let request = ClipboardRequest::capture(&editor);
        editor
            .set_selection(Selection::caret(ByteOffset(3)))
            .unwrap();
        let mut clipboard = MemoryClipboard::default();
        assert!(request.publish(&mut editor, &mut clipboard, true).is_err());
        assert_eq!(clipboard.writes, 0);
        let request = ClipboardRequest::capture(&editor);
        let mut other = Editor::new("abc");
        assert!(request.publish(&mut other, &mut clipboard, true).is_err());
        assert_eq!(clipboard.writes, 0);
    }

    #[test]
    fn copy_publishes_exact_source_without_history_or_revision_changes() {
        let mut editor = Editor::new("[é](url)\n");
        editor
            .set_selection(Selection {
                anchor: ByteOffset(0),
                head: ByteOffset(9),
                ..Selection::default()
            })
            .unwrap();
        let initial = editor.snapshot();
        let mut clipboard = MemoryClipboard::default();
        assert_eq!(
            ClipboardRequest::capture(&editor)
                .publish(&mut editor, &mut clipboard, false)
                .unwrap(),
            ClipboardResult::Copied
        );
        assert_eq!(clipboard.text, "[é](url)");
        assert_eq!(editor.snapshot().revision(), initial.revision());
        assert!(!editor.can_undo());
    }
}
