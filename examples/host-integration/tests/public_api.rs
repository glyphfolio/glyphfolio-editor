use glyphfolio_editor_core::{
    ByteOffset, EditCommand, Editor, HeadingLevel, Selection, Transaction, TransactionOrigin,
};
use glyphfolio_editor_host_example::{reopen_utf8, save_snapshot_atomic};

#[test]
fn generic_host_can_embed_and_control_the_engine_without_ui_or_storage_internals() {
    let report = glyphfolio_editor_host_example::exercise_public_api().unwrap();
    assert_eq!(report.revision, 6);
    assert_eq!(report.committed_events, 6);
    assert!(report.clean);
}

#[test]
fn host_can_apply_notifications_to_its_own_mirror_in_the_declared_coordinate_space() {
    let mut editor = Editor::new("é abc 日本 xyz");
    let notifications = editor.subscribe();
    let before = editor.snapshot();
    let mut mirror = before.text();
    editor
        .apply(
            Transaction::new(&before)
                .replace(ByteOffset(3)..ByteOffset(6), "longer")
                .replace(ByteOffset(14)..ByteOffset(17), "終")
                .with_selection(Selection::caret(ByteOffset(9))),
        )
        .unwrap();
    let event = notifications.try_recv().unwrap();
    for change in event.changes.iter().rev() {
        mirror.replace_range(change.range.start.0..change.range.end.0, &change.inserted);
    }
    assert_eq!(mirror, editor.snapshot().text());
    assert_eq!(mirror, event.snapshot.text());
    assert_eq!(event.selection_after, editor.selection());
}

#[test]
fn host_saves_actual_bytes_and_reopens_after_edit_undo_without_serialization() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("note.md");
    let original = "\u{feff}# note\r\n\r\n日本 😀  \r\n";
    let mut editor = Editor::new(original);
    editor
        .execute(EditCommand::SetHeading(Some(HeadingLevel::Three)))
        .unwrap()
        .unwrap();
    editor.undo().unwrap().unwrap();
    let save = save_snapshot_atomic(editor.snapshot(), &path).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
    assert!(save.acknowledge(&mut editor).unwrap());
    assert!(!editor.is_dirty());
    let reopened = reopen_utf8(&path).unwrap();
    assert_eq!(reopened.snapshot().text(), original);
    assert_ne!(reopened.snapshot().document_id(), save.document_id());
    assert!(!reopened.can_undo());
    assert!(!reopened.is_dirty());
    assert!(save.acknowledge(&mut Editor::new(original)).is_err());
}

#[test]
fn host_atomic_replacement_and_delayed_acknowledgement_preserve_newer_edits() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("note.md");
    std::fs::write(&path, "old file bytes").unwrap();
    let mut editor = Editor::new("first\r\n");
    let first = save_snapshot_atomic(editor.snapshot(), &path).unwrap();
    editor
        .set_selection(Selection::caret(ByteOffset(editor.snapshot().byte_len())))
        .unwrap();
    editor
        .insert_text("second\r\n", TransactionOrigin::Typing)
        .unwrap();
    assert!(first.acknowledge(&mut editor).unwrap());
    assert!(
        editor.is_dirty(),
        "completed older save cannot acknowledge newer text"
    );
    assert_eq!(reopen_utf8(&path).unwrap().snapshot().text(), "first\r\n");
    let latest = save_snapshot_atomic(editor.snapshot(), &path).unwrap();
    assert!(latest.acknowledge(&mut editor).unwrap());
    assert!(!editor.is_dirty());
    assert!(
        !first.acknowledge(&mut editor).unwrap(),
        "reordered acknowledgements are ignored"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"first\r\nsecond\r\n");
    assert_eq!(
        std::fs::read_dir(directory.path()).unwrap().count(),
        1,
        "successful atomic save leaves no staging file"
    );
}

#[test]
fn failed_host_save_does_not_publish_a_receipt_or_mark_source_clean() {
    let directory = tempfile::tempdir().unwrap();
    let mut editor = Editor::new("saved");
    editor
        .insert_text("edited", TransactionOrigin::Typing)
        .unwrap();
    let before = editor.snapshot();
    let selection = editor.selection();
    let destination = directory.path().join("missing").join("note.md");
    assert!(save_snapshot_atomic(before.clone(), &destination).is_err());
    assert!(editor.is_dirty());
    assert_eq!(editor.snapshot().revision(), before.revision());
    assert_eq!(editor.snapshot().text(), before.text());
    assert_eq!(editor.selection(), selection);
    let invalid = directory.path().join("invalid.md");
    std::fs::write(&invalid, [0xff, 0xfe, 0x00]).unwrap();
    assert_eq!(
        reopen_utf8(invalid).err().unwrap().kind(),
        std::io::ErrorKind::InvalidData
    );
}
