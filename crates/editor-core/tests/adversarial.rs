//! Independent public-API regressions for transaction and history edge cases.
//! These intentionally avoid storage internals and random-test implementation details.

use glyphfolio_editor_core::{
    Affinity, ByteOffset, Editor, Selection, Transaction, TransactionOrigin,
};
use unicode_segmentation::UnicodeSegmentation;

fn replace(editor: &mut Editor, from: usize, to: usize, inserted: &str) {
    let transaction = Transaction::new(&editor.snapshot())
        .replace(ByteOffset(from)..ByteOffset(to), inserted)
        .with_origin(TransactionOrigin::Programmatic);
    editor.apply(transaction).expect("valid source edit");
}

#[test]
fn rejected_multibyte_edit_is_atomic_and_does_not_publish_a_change() {
    let mut editor = Editor::new("é one\r\n二 two\n");
    let initial = editor.snapshot();
    let events = editor.subscribe();
    let transaction = Transaction::new(&initial)
        .replace(ByteOffset(1)..ByteOffset(1), "INVALID")
        .replace(
            ByteOffset(initial.byte_len())..ByteOffset(initial.byte_len()),
            "valid",
        );

    assert!(editor.apply(transaction).is_err());
    assert_eq!(editor.snapshot().text(), initial.text());
    assert_eq!(editor.snapshot().revision(), initial.revision());
    assert!(!editor.can_undo());
    assert!(events.try_recv().is_err());
}

#[test]
fn invalid_post_edit_selection_rejects_the_entire_transaction() {
    let mut editor = Editor::new("abc");
    let before = editor.snapshot();
    let transaction = Transaction::new(&before)
        .replace(ByteOffset(0)..ByteOffset(0), "é")
        .with_selection(Selection::caret(ByteOffset(1)));

    assert!(editor.apply(transaction).is_err());
    assert_eq!(editor.snapshot().text(), "abc");
    assert_eq!(editor.snapshot().revision(), before.revision());
    assert_eq!(editor.selection(), Selection::caret(ByteOffset(0)));
}

#[test]
fn unordered_multi_changes_use_one_base_snapshot_and_inverse_lengths() {
    let mut editor = Editor::new("0123456789");
    let before = editor.snapshot();
    let transaction = Transaction::new(&before)
        .replace(ByteOffset(6)..ByteOffset(10), "Z")
        .replace(ByteOffset(0)..ByteOffset(1), "")
        .replace(ByteOffset(3)..ByteOffset(4), "XX")
        .with_selection(Selection::caret(ByteOffset(7)));

    editor.apply(transaction).unwrap();
    assert_eq!(editor.snapshot().text(), "12XX45Z");
    let edited_revision = editor.snapshot().revision();
    editor.undo().unwrap().expect("one undo step");
    assert_eq!(editor.snapshot().text(), "0123456789");
    assert!(editor.snapshot().revision() > edited_revision);
    editor.redo().unwrap().expect("one redo step");
    assert_eq!(editor.snapshot().text(), "12XX45Z");
    assert_eq!(editor.selection(), Selection::caret(ByteOffset(7)));
}

#[test]
fn adjacent_deletions_have_an_unambiguous_combined_inverse() {
    let mut editor = Editor::new("abcd");
    let transaction = Transaction::new(&editor.snapshot())
        .replace(ByteOffset(0)..ByteOffset(1), "")
        .replace(ByteOffset(1)..ByteOffset(2), "");
    editor.apply(transaction).unwrap();
    assert_eq!(editor.snapshot().text(), "cd");
    editor.undo().unwrap().expect("adjacent deletions undo");
    assert_eq!(editor.snapshot().text(), "abcd");
    editor.redo().unwrap().expect("adjacent deletions redo");
    assert_eq!(editor.snapshot().text(), "cd");
}

#[test]
fn snapshots_remain_immutable_and_cannot_be_reused_after_undo() {
    let mut editor = Editor::new("αβ");
    let original = editor.snapshot();
    let stale = Transaction::new(&original).replace(ByteOffset(0)..ByteOffset(0), "stale");
    replace(&mut editor, 0, 0, "X");
    editor.undo().unwrap().unwrap();

    assert_eq!(original.text(), "αβ");
    assert_eq!(editor.snapshot().text(), original.text());
    assert!(editor.snapshot().revision() > original.revision());
    assert!(editor.apply(stale).is_err());
    assert_eq!(editor.snapshot().text(), "αβ");
}

#[test]
fn identical_bytes_do_not_allow_cross_document_transactions_or_save_acknowledgements() {
    let mut first = Editor::new("same");
    let second = Editor::new("same");
    let foreign = second.snapshot();
    let initial = first.snapshot();
    let foreign_transaction =
        Transaction::new(&foreign).replace(ByteOffset(0)..ByteOffset(0), "wrong document");

    assert_ne!(initial.document_id(), foreign.document_id());
    assert!(first.apply(foreign_transaction).is_err());
    assert!(first.acknowledge_saved(&foreign).is_err());
    assert_eq!(first.snapshot().text(), initial.text());
    assert_eq!(first.snapshot().revision(), initial.revision());
}

#[test]
fn reordered_save_acknowledgements_cannot_overwrite_the_latest_saved_state() {
    let mut editor = Editor::new("zero");
    replace(&mut editor, 0, 4, "one");
    let save_one = editor.snapshot();
    replace(&mut editor, 0, 3, "two");
    let save_two = editor.snapshot();

    assert!(editor.acknowledge_saved(&save_two).unwrap());
    assert!(!editor.is_dirty());
    assert!(!editor.acknowledge_saved(&save_one).unwrap());
    assert!(!editor.is_dirty());

    editor.undo().unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), "one");
    assert!(editor.is_dirty());
    editor.redo().unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), "two");
    assert!(!editor.is_dirty());
}

#[test]
fn acknowledging_an_in_flight_save_does_not_mark_newer_typing_clean() {
    let mut editor = Editor::new("zero");
    replace(&mut editor, 0, 4, "saved");
    let in_flight = editor.snapshot();
    replace(&mut editor, 5, 5, "+newer");

    assert!(editor.acknowledge_saved(&in_flight).unwrap());
    assert!(editor.is_dirty());
    editor.undo().unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), "saved");
    assert!(!editor.is_dirty());
    assert!(editor.snapshot().revision() > in_flight.revision());
}

#[test]
fn new_edits_after_undo_invalidate_only_the_redo_branch() {
    let mut editor = Editor::new("a");
    replace(&mut editor, 1, 1, "b");
    replace(&mut editor, 2, 2, "c");
    editor.undo().unwrap().unwrap();
    assert!(editor.can_redo());
    replace(&mut editor, 2, 2, "d");

    assert_eq!(editor.snapshot().text(), "abd");
    assert!(!editor.can_redo());
    assert!(editor.redo().unwrap().is_none());
    editor.undo().unwrap().unwrap();
    editor.undo().unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), "a");
}

#[test]
fn raw_markdown_bom_mixed_endings_and_trailing_spaces_survive_undo() {
    let initial = "\u{feff}# Heading\r\n\r\n__strong__ &amp; \\*  \n> 二\r\n\n";
    let mut editor = Editor::new(initial);
    replace(&mut editor, initial.len(), initial.len(), "👩🏽‍💻");
    editor.undo().unwrap().unwrap();

    assert_eq!(editor.snapshot().text().as_bytes(), initial.as_bytes());
    assert_eq!(editor.snapshot().byte_len(), initial.len());
}

#[test]
fn zero_history_budget_still_commits_without_retaining_an_undo_step() {
    let mut editor = Editor::new("a").with_history_limit(0);
    replace(&mut editor, 1, 1, "b");

    assert_eq!(editor.snapshot().text(), "ab");
    assert_eq!(editor.history_bytes(), 0);
    assert!(!editor.can_undo());
    assert!(editor.undo().unwrap().is_none());
}

#[test]
fn history_memory_stays_bounded_while_moving_entries_between_stacks() {
    let budget = 1024;
    let mut editor = Editor::new("").with_history_limit(budget);
    for _ in 0..40 {
        let end = editor.snapshot().byte_len();
        replace(&mut editor, end, end, "abcdefgh");
        assert!(editor.history_bytes() <= budget);
    }
    while editor.can_undo() {
        editor.undo().unwrap().unwrap();
        assert!(editor.history_bytes() <= budget);
    }
    while editor.can_redo() {
        editor.redo().unwrap().unwrap();
        assert!(editor.history_bytes() <= budget);
    }
}

#[test]
fn a_grapheme_larger_than_rope_chunks_moves_as_one_user_unit() {
    let prefix = "a".repeat(2048);
    let cluster = format!("e{}", "\u{301}".repeat(1500));
    let initial = format!("{prefix}{cluster}z");
    let mut editor = Editor::new(&initial);
    editor
        .set_selection(Selection::caret(ByteOffset(prefix.len())))
        .unwrap();

    editor.move_grapheme(true, false).unwrap();
    assert_eq!(
        editor.selection().head,
        ByteOffset(prefix.len() + cluster.len())
    );
    editor.move_grapheme(false, false).unwrap();
    assert_eq!(editor.selection().head, ByteOffset(prefix.len()));

    editor.move_grapheme(true, true).unwrap();
    editor
        .insert_text("", TransactionOrigin::Programmatic)
        .unwrap();
    assert_eq!(editor.snapshot().text(), format!("{prefix}z"));
    editor.undo().unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), initial);
}

#[test]
fn deletion_from_an_interior_scalar_caret_removes_a_whole_grapheme() {
    for forward in [false, true] {
        let mut editor = Editor::new("e\u{301}x");
        // Valid UTF-8 boundary between the base letter and combining accent,
        // as a programmatic or accessibility caller may supply.
        editor
            .set_selection(Selection::caret(ByteOffset(1)))
            .unwrap();
        if forward {
            editor.delete_forward().unwrap().unwrap();
        } else {
            editor.delete_backward().unwrap().unwrap();
        }
        assert_eq!(editor.snapshot().text(), "x");
        assert_eq!(editor.selection(), Selection::caret(ByteOffset(0)));
        editor.undo().unwrap().unwrap();
        assert_eq!(editor.snapshot().text(), "e\u{301}x");
    }
}

#[test]
fn user_deletion_expands_partial_grapheme_selections_in_both_directions() {
    for (anchor, head) in [(1, 3), (3, 1)] {
        let mut editor = Editor::new("e\u{301}x");
        editor
            .set_selection(Selection {
                anchor: ByteOffset(anchor),
                head: ByteOffset(head),
                affinity: Affinity::After,
            })
            .unwrap();
        editor.delete_backward().unwrap().unwrap();
        assert_eq!(editor.snapshot().text(), "x");
        editor.undo().unwrap().unwrap();
        assert_eq!(editor.snapshot().text(), "e\u{301}x");
        assert_eq!(editor.selection().anchor, ByteOffset(anchor));
        assert_eq!(editor.selection().head, ByteOffset(head));
    }
}
#[test]
fn user_insertions_join_following_marks_without_leaving_an_interior_caret() {
    use glyphfolio_editor_core::{
        Affinity, ByteOffset, Editor, Selection, Transaction, TransactionOrigin,
    };
    for (initial, inserted) in [("\u{301}next", "a"), ("👩next", "👩\u{200d}")] {
        let mut editor = Editor::new(initial);
        let before = editor.selection();
        let event = editor
            .insert_text(inserted, TransactionOrigin::Typing)
            .unwrap();
        let text = editor.snapshot().text();
        let expected = text
            .grapheme_indices(true)
            .nth(1)
            .map_or(text.len(), |(at, _)| at);
        assert_eq!(editor.selection().head, ByteOffset(expected));
        assert_eq!(event.selection_after, editor.selection());
        editor.undo().unwrap();
        assert_eq!(editor.snapshot().text(), initial);
        assert_eq!(editor.selection(), before);
        editor.redo().unwrap();
        assert_eq!(editor.selection().head, ByteOffset(expected));
    }
    let mut raw = Editor::new("\u{301}");
    raw.apply(
        Transaction::new(&raw.snapshot())
            .replace(ByteOffset(0)..ByteOffset(0), "a")
            .with_selection(Selection::caret(ByteOffset(1))),
    )
    .unwrap();
    assert_eq!(
        raw.selection().head,
        ByteOffset(1),
        "raw scalar transaction remains explicit"
    );
    assert_eq!(
        raw.snapshot()
            .snap_grapheme(ByteOffset(1), Affinity::Before)
            .unwrap(),
        ByteOffset(0)
    );
    assert_eq!(
        raw.snapshot()
            .snap_grapheme(ByteOffset(1), Affinity::After)
            .unwrap(),
        ByteOffset(3)
    );
}
