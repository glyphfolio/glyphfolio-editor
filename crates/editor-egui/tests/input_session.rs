//! Controller contract tests independent of a native platform transport.
//! The path points at the exact production module while root integrates it.
#[path = "../src/input.rs"]
mod input;

use glyphfolio_editor_core::{ByteOffset, Editor, PresentationMode, Selection, TransactionOrigin};
use input::{CompositionSession, InputError};

fn caret(editor: &mut Editor, byte: usize) {
    editor
        .set_selection(Selection::caret(ByteOffset(byte)))
        .unwrap();
}

#[test]
fn fresh_explicit_epoch_accepts_commit_only_emoji_after_cancellation() {
    let mut editor = Editor::new("hello ");
    caret(&mut editor, 6);
    let mut session = CompositionSession::default();
    let old = session.begin_input(&editor).unwrap();
    session.preedit(&editor, old, "pending", None).unwrap();
    session.interrupt();
    assert!(session.can_close());
    assert_eq!(session.epoch(), None);
    assert!(matches!(
        session.commit(&mut editor, old, "late"),
        Err(InputError::InactiveEpoch)
    ));

    // This is an explicit new host/user-input boundary, not automatic rearming
    // simply because an untagged Commit event arrived.
    let fresh = session.begin_input(&editor).unwrap();
    assert!(fresh.diagnostic_id() > old.diagnostic_id());
    assert!(matches!(
        session.commit(&mut editor, old, "late"),
        Err(InputError::StaleEpoch)
    ));
    assert_eq!(session.epoch(), Some(fresh));
    session.commit(&mut editor, fresh, "👩‍👩‍👧‍👦").unwrap();
    assert_eq!(editor.snapshot().text(), "hello 👩‍👩‍👧‍👦");
    editor.undo().unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), "hello ");
}

#[test]
fn epoch_can_accept_successive_commit_only_input_without_preedit() {
    let mut editor = Editor::new("");
    let mut session = CompositionSession::default();
    let epoch = session.begin_input(&editor).unwrap();
    session.commit(&mut editor, epoch, "😀").unwrap();
    session.commit(&mut editor, epoch, "é").unwrap();
    assert_eq!(editor.snapshot().text(), "😀é");
    assert!(!session.is_composing());
    assert!(session.can_close());
}

#[test]
fn empty_preedit_retains_selection_and_close_mode_barriers_until_commit() {
    let mut editor = Editor::new("a SELECT z");
    editor
        .set_selection(Selection {
            anchor: ByteOffset(2),
            head: ByteOffset(8),
            ..Selection::default()
        })
        .unwrap();
    let mut session = CompositionSession::default();
    let epoch = session.begin_input(&editor).unwrap();
    session
        .preedit(&editor, epoch, "にほん", Some(1..2))
        .unwrap();
    assert!(session.is_composing());
    let projection = session
        .display(&editor.snapshot(), editor.selection())
        .unwrap();
    assert_eq!(projection.text, "a にほん z");
    assert_eq!(projection.caret_byte, 2 + "にほ".len());
    assert_eq!(projection.preedit_range, Some(2..2 + "にほん".len()));
    assert_eq!(editor.snapshot().text(), "a SELECT z");

    session.request_presentation(PresentationMode::Rich);
    session.preedit(&editor, epoch, "", None).unwrap();
    assert!(!session.can_close());
    assert_eq!(session.presentation(), PresentationMode::Source);
    assert_eq!(
        session
            .display(&editor.snapshot(), editor.selection())
            .unwrap()
            .text,
        "a SELECT z"
    );
    session.commit(&mut editor, epoch, "日本").unwrap();
    assert!(session.can_close());
    assert_eq!(session.presentation(), PresentationMode::Rich);
    assert_eq!(editor.snapshot().text(), "a 日本 z");
    editor.undo().unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), "a SELECT z");
    assert!(!editor.can_undo());
}

#[test]
fn dismissed_preedit_does_not_follow_a_same_revision_caret_change() {
    let mut editor = Editor::new("abcdef");
    caret(&mut editor, 1);
    let original = editor.snapshot();
    let mut session = CompositionSession::default();
    let epoch = session.begin_input(&editor).unwrap();
    session.preedit(&editor, epoch, "pending", None).unwrap();
    session.preedit(&editor, epoch, "", None).unwrap();
    caret(&mut editor, 6);
    assert!(matches!(
        session.commit(&mut editor, epoch, "WRONG"),
        Err(InputError::OwnershipChanged)
    ));
    assert_eq!(editor.snapshot().text(), original.text());
    assert_eq!(editor.snapshot().revision(), original.revision());
    assert_eq!(editor.selection(), Selection::caret(ByteOffset(6)));
    assert!(!editor.can_undo());
}

#[test]
fn host_document_replacement_invalidates_old_native_owner() {
    let mut editor = Editor::new("A");
    let mut session = CompositionSession::default();
    let old = session.begin_input(&editor).unwrap();
    session.preedit(&editor, old, "pending", None).unwrap();
    editor = Editor::new("B");
    assert_eq!(
        session.synchronize(&editor),
        Err(InputError::OwnershipChanged)
    );
    assert!(matches!(
        session.commit(&mut editor, old, "wrong"),
        Err(InputError::InactiveEpoch)
    ));
    assert_eq!(editor.snapshot().text(), "B");
    assert!(!editor.can_undo());
}

#[test]
fn host_revision_change_invalidates_old_native_owner_without_undoing_host_edit() {
    let mut editor = Editor::new("A");
    let mut session = CompositionSession::default();
    let old = session.begin_input(&editor).unwrap();
    session.preedit(&editor, old, "pending", None).unwrap();
    editor
        .insert_text("host", TransactionOrigin::Programmatic)
        .unwrap();
    let committed = editor.snapshot();
    assert!(matches!(
        session.commit(&mut editor, old, "wrong"),
        Err(InputError::OwnershipChanged)
    ));
    assert_eq!(editor.snapshot().text(), committed.text());
    assert_eq!(editor.snapshot().revision(), committed.revision());
}

#[test]
fn invalid_preedit_scalar_range_does_not_replace_valid_overlay() {
    let editor = Editor::new("source");
    let mut session = CompositionSession::default();
    let epoch = session.begin_input(&editor).unwrap();
    session.preedit(&editor, epoch, "é中", Some(0..2)).unwrap();
    let before = session
        .display(&editor.snapshot(), editor.selection())
        .unwrap();
    assert_eq!(
        session.preedit(&editor, epoch, "a", Some(0..2)),
        Err(InputError::InvalidPreeditRange)
    );
    assert_eq!(
        session.preedit(
            &editor,
            epoch,
            "a",
            Some(std::ops::Range { start: 1, end: 0 })
        ),
        Err(InputError::InvalidPreeditRange)
    );
    assert_eq!(
        session
            .display(&editor.snapshot(), editor.selection())
            .unwrap(),
        before
    );
    assert!(!editor.can_undo());
}

#[test]
fn surrounding_deletion_during_preedit_commits_as_one_composition() {
    let mut editor = Editor::new("Aé中Z");
    caret(&mut editor, "Aé中".len());
    let mut session = CompositionSession::default();
    let epoch = session.begin_input(&editor).unwrap();
    session.preedit(&editor, epoch, "日本", None).unwrap();
    assert!(
        session
            .delete_surrounding(&mut editor, epoch, 2, 0)
            .unwrap()
            .is_none()
    );
    assert_eq!(editor.snapshot().text(), "Aé中Z");
    assert_eq!(
        session
            .display(&editor.snapshot(), editor.selection())
            .unwrap()
            .text,
        "A日本Z"
    );
    session.commit(&mut editor, epoch, "日本").unwrap();
    assert_eq!(editor.snapshot().text(), "A日本Z");
    editor.undo().unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), "Aé中Z");
    assert!(!editor.can_undo());
}

#[test]
fn cancelling_staged_surrounding_deletion_persists_nothing() {
    let mut editor = Editor::new("Aé中Z");
    caret(&mut editor, "Aé中".len());
    let initial = editor.snapshot();
    let mut session = CompositionSession::default();
    let epoch = session.begin_input(&editor).unwrap();
    session.preedit(&editor, epoch, "pending", None).unwrap();
    session
        .delete_surrounding(&mut editor, epoch, 2, 0)
        .unwrap();
    session.request_presentation(PresentationMode::Rich);
    session.interrupt();
    assert_eq!(session.presentation(), PresentationMode::Rich);
    assert!(session.can_close());
    assert_eq!(editor.snapshot().text(), initial.text());
    assert_eq!(editor.snapshot().revision(), initial.revision());
    assert!(!editor.can_undo());
}

#[test]
fn empty_commit_cancels_provisional_text_and_surrounding_delete() {
    let mut editor = Editor::new("Aé中Z");
    caret(&mut editor, "Aé中".len());
    let initial = editor.snapshot();
    let mut session = CompositionSession::default();
    let epoch = session.begin_input(&editor).unwrap();
    session.preedit(&editor, epoch, "pending", None).unwrap();
    session
        .delete_surrounding(&mut editor, epoch, 2, 0)
        .unwrap();
    assert!(session.commit(&mut editor, epoch, "").unwrap().is_none());
    assert!(session.can_close());
    assert_eq!(editor.snapshot().text(), initial.text());
    assert_eq!(editor.snapshot().revision(), initial.revision());
}

#[test]
fn standalone_surrounding_delete_counts_scalars_and_refreshes_owner() {
    let mut editor = Editor::new("Aé中Z");
    caret(&mut editor, "Aé中".len());
    let mut session = CompositionSession::default();
    let epoch = session.begin_input(&editor).unwrap();
    session
        .delete_surrounding(&mut editor, epoch, 2, 0)
        .unwrap()
        .unwrap();
    assert_eq!(editor.snapshot().text(), "AZ");
    assert_eq!(editor.selection(), Selection::caret(ByteOffset(1)));
    session.commit(&mut editor, epoch, "😀").unwrap();
    assert_eq!(editor.snapshot().text(), "A😀Z");
}

#[test]
fn surrounding_counts_clamp_at_source_edges_and_zero_is_noop() {
    let mut editor = Editor::new("é中");
    caret(&mut editor, 2);
    let mut session = CompositionSession::default();
    let epoch = session.begin_input(&editor).unwrap();
    assert!(
        session
            .delete_surrounding(&mut editor, epoch, 0, 0)
            .unwrap()
            .is_none()
    );
    session
        .delete_surrounding(&mut editor, epoch, usize::MAX, usize::MAX)
        .unwrap()
        .unwrap();
    assert_eq!(editor.snapshot().text(), "");
    editor.undo().unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), "é中");
}

#[test]
fn identical_committed_replacement_collapses_selection_without_content_revision() {
    let mut editor = Editor::new("same");
    editor
        .set_selection(Selection {
            anchor: ByteOffset(0),
            head: ByteOffset(4),
            ..Selection::default()
        })
        .unwrap();
    let initial = editor.snapshot();
    let mut session = CompositionSession::default();
    let epoch = session.begin_input(&editor).unwrap();
    assert!(
        session
            .commit(&mut editor, epoch, "same")
            .unwrap()
            .is_none()
    );
    assert_eq!(editor.snapshot().text(), initial.text());
    assert_eq!(editor.snapshot().revision(), initial.revision());
    assert_eq!(editor.selection(), Selection::caret(ByteOffset(4)));
    assert!(!editor.can_undo());
}

#[test]
fn duplicate_text_suppression_is_one_event_and_can_be_cleared() {
    let mut editor = Editor::new("");
    let mut session = CompositionSession::default();
    let epoch = session.begin_input(&editor).unwrap();
    session.commit(&mut editor, epoch, "a").unwrap();
    assert!(session.should_suppress_duplicate_text("a"));
    assert!(!session.should_suppress_duplicate_text("a"));
    session.commit(&mut editor, epoch, "b").unwrap();
    session.clear_duplicate();
    assert!(!session.should_suppress_duplicate_text("b"));
    session.commit(&mut editor, epoch, "c").unwrap();
    assert!(!session.should_suppress_duplicate_text("not c"));
    assert!(!session.should_suppress_duplicate_text("c"));
}

#[test]
fn plain_display_never_exposes_a_second_authoritative_document() {
    let editor = Editor::new("verbatim **source**");
    let mut session = CompositionSession::default();
    session.request_presentation(PresentationMode::Rich);
    let projection = session
        .display(&editor.snapshot(), editor.selection())
        .unwrap();
    assert_eq!(projection.text, editor.snapshot().text());
    assert_eq!(projection.caret_byte, 0);
    assert_eq!(projection.preedit_range, None);
    assert_eq!(session.presentation(), PresentationMode::Rich);
}
#[test]
fn committed_base_character_joins_following_mark_with_a_legal_caret() {
    let mut editor = Editor::new("\u{301}next");
    let mut session = CompositionSession::default();
    let epoch = session.begin_input(&editor).unwrap();
    session.preedit(&editor, epoch, "a", None).unwrap();
    session.commit(&mut editor, epoch, "a").unwrap();
    assert_eq!(editor.snapshot().text(), "a\u{301}next");
    assert_eq!(editor.selection(), Selection::caret(ByteOffset(3)));
    editor.undo().unwrap();
    assert_eq!(editor.snapshot().text(), "\u{301}next");
}
