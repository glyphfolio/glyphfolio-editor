//! Generic host design fixture, using only the original engine's public API.

use glyphfolio_editor_core::{
    ByteOffset, DocumentId, Editor, EditorError, PresentationMode, Revision, Selection, Snapshot,
    Transaction, ViewState,
};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

/// Completion of a host-owned write, tagged with the exact immutable snapshot.
/// Constructed only after the bytes, rename, and available sync steps succeed.
#[derive(Clone, Debug)]
pub struct SaveReceipt {
    path: PathBuf,
    snapshot: Snapshot,
}

impl SaveReceipt {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn document_id(&self) -> DocumentId {
        self.snapshot.document_id()
    }
    pub fn revision(&self) -> Revision {
        self.snapshot.revision()
    }
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }
    pub fn acknowledge(&self, editor: &mut Editor) -> Result<bool, EditorError> {
        editor.acknowledge_saved(&self.snapshot)
    }
}

/// Example host save service. IO stays outside the editor engine.
///
/// A same-directory temporary file is written verbatim and synchronized before
/// atomic replacement. On Unix the parent directory is synchronized as well.
/// The caller owns the destination and any external-file conflict policy; this
/// example never creates missing directories or silently decodes invalid UTF-8.
pub fn save_snapshot_atomic(snapshot: Snapshot, path: impl AsRef<Path>) -> io::Result<SaveReceipt> {
    let path = path.as_ref();
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    for chunk in snapshot.chunks() {
        temporary.write_all(chunk.as_bytes())?;
    }
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    let persisted = temporary.persist(path).map_err(|error| error.error)?;
    persisted.sync_all()?;
    #[cfg(unix)]
    std::fs::File::open(parent)?.sync_all()?;
    Ok(SaveReceipt {
        path: path.to_owned(),
        snapshot,
    })
}

/// Load the host's accepted UTF-8 bytes without normalizing Markdown or endings.
/// A reopened file always creates a fresh document session and fresh history.
pub fn reopen_utf8(path: impl AsRef<Path>) -> io::Result<Editor> {
    std::fs::read_to_string(path).map(|text| Editor::new(&text))
}

/// Observable results without exposing storage or presentation internals.
#[derive(Debug)]
pub struct HostReport {
    pub revision: u64,
    pub committed_events: usize,
    pub bytes: usize,
    pub clean: bool,
}

/// Exercise create, subscribe, transact, view preferences, save, undo, and redo.
///
/// `Rich` below is view metadata only; this example does not implement rich layout.
pub fn exercise_public_api() -> Result<HostReport, EditorError> {
    let original = "\u{feff}# Host fixture\r\n\r\nHello";
    let mut editor = Editor::new(original);
    let initial = editor.snapshot();
    let changes = editor.subscribe();
    let mut view = ViewState::default();

    let end = initial.byte_len();
    editor.apply(
        Transaction::new(&initial)
            .replace(ByteOffset(end)..ByteOffset(end), " Rust")
            .with_selection(Selection::caret(ByteOffset(end + 5))),
    )?;
    let in_flight_save = editor.snapshot();
    let selected = editor.selection();
    view.set_presentation(PresentationMode::Rich);
    view.set_presentation(PresentationMode::Source);
    assert_eq!(editor.snapshot().revision(), in_flight_save.revision());
    assert_eq!(editor.selection(), selected);

    // A second edit while an older snapshot is being persisted.
    let end = in_flight_save.byte_len();
    editor
        .apply(Transaction::new(&in_flight_save).replace(ByteOffset(end)..ByteOffset(end), "!"))?;
    assert!(editor.acknowledge_saved(&in_flight_save)?);
    assert!(
        editor.is_dirty(),
        "an older save must not mark newer text clean"
    );
    let persisted = editor.snapshot();
    assert!(editor.acknowledge_saved(&persisted)?);
    assert!(!editor.is_dirty());

    let unrelated = Editor::new(&persisted.text()).snapshot();
    assert!(editor.acknowledge_saved(&unrelated).is_err());

    editor.undo()?.expect("newer edit has history");
    editor.undo()?.expect("initial transaction has history");
    assert_eq!(editor.snapshot().text(), original);
    assert_eq!(initial.text(), original, "opening snapshot is immutable");
    editor.redo()?.expect("first transaction can be redone");
    editor.redo()?.expect("second transaction can be redone");
    assert_eq!(editor.snapshot().text(), persisted.text());
    assert!(
        !editor.is_dirty(),
        "redo returns to the saved content state"
    );

    let observed: Vec<_> = changes.try_iter().collect();
    assert_eq!(
        observed.len(),
        6,
        "views and save acknowledgements are not edits"
    );
    for (index, event) in observed.iter().enumerate() {
        assert_eq!(event.document_id, initial.document_id());
        assert_eq!(event.before_revision.0, index as u64);
        assert_eq!(event.revision.0, index as u64 + 1);
        assert_eq!(event.snapshot.revision(), event.revision);
    }

    Ok(HostReport {
        revision: editor.snapshot().revision().0,
        committed_events: observed.len(),
        bytes: editor.snapshot().byte_len(),
        clean: !editor.is_dirty(),
    })
}
