use std::sync::mpsc::TryRecvError;

use glyphfolio_editor_core::{
    Affinity, ByteOffset, Editor, EditorError, Revision, ScalarIndex, Selection, Transaction,
    TransactionOrigin,
};
use proptest::prelude::*;

fn boundary(text: &str, index: usize) -> usize {
    let boundaries: Vec<_> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain([text.len()])
        .collect();
    boundaries[index % boundaries.len()]
}

fn apply_event(text: &mut String, changes: &[glyphfolio_editor_core::TextChange]) {
    for change in changes.iter().rev() {
        text.replace_range(change.range.start.0..change.range.end.0, &change.inserted);
    }
}

#[test]
fn multi_edit_inverse_and_observer_changes_use_their_actual_base_revision() {
    let mut editor = Editor::new("aé中xyz");
    let initial = editor.snapshot();
    let event = editor
        .apply(
            Transaction::new(&initial)
                .replace(ByteOffset(0)..ByteOffset(1), "**")
                .replace(ByteOffset(3)..ByteOffset(6), "Q")
                .replace(ByteOffset(7)..ByteOffset(8), "長い"),
        )
        .unwrap();
    let mut mirror = initial.text();
    apply_event(&mut mirror, &event.changes);
    assert_eq!(mirror, editor.snapshot().text());
    let undo = editor.undo().unwrap().unwrap();
    assert_eq!(undo.before_revision, event.revision);
    assert_eq!(undo.revision, Revision(2));
    apply_event(&mut mirror, &undo.changes);
    assert_eq!(mirror, initial.text());
    let redo = editor.redo().unwrap().unwrap();
    apply_event(&mut mirror, &redo.changes);
    assert_eq!(mirror, event.snapshot.text());
}

#[test]
fn coalesced_typing_is_one_step_but_cursor_discontinuity_is_a_boundary() {
    let mut editor = Editor::new("");
    let first = editor.insert_text("é", TransactionOrigin::Typing).unwrap();
    editor.insert_text("中", TransactionOrigin::Typing).unwrap();
    let typed = editor.snapshot();
    let undo = editor.undo().unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), "");
    assert_eq!(editor.selection(), Selection::caret(ByteOffset(0)));
    assert!(!editor.can_undo());
    let mut mirror = typed.text();
    apply_event(&mut mirror, &undo.changes);
    assert_eq!(mirror, "");
    editor.redo().unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), "é中");
    assert_eq!(editor.selection().head, ByteOffset(5));
    editor
        .set_selection(Selection::caret(ByteOffset(0)))
        .unwrap();
    editor
        .set_selection(Selection::caret(ByteOffset(5)))
        .unwrap();
    editor.insert_text("!", TransactionOrigin::Typing).unwrap();
    editor.undo().unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), "é中");
    assert!(editor.can_undo());
    assert_eq!(first.snapshot.text(), "é");
}

#[test]
fn paste_composition_and_newline_never_join_typing() {
    for origin in [
        TransactionOrigin::Paste,
        TransactionOrigin::Composition,
        TransactionOrigin::Formatting,
    ] {
        let mut editor = Editor::new("");
        editor.insert_text("a", TransactionOrigin::Typing).unwrap();
        editor.insert_text("日本", origin).unwrap();
        editor.insert_text("b", TransactionOrigin::Typing).unwrap();
        editor.undo().unwrap();
        assert_eq!(editor.snapshot().text(), "a日本");
        editor.undo().unwrap();
        assert_eq!(editor.snapshot().text(), "a");
    }
    let mut editor = Editor::new("");
    editor.insert_text("a", TransactionOrigin::Typing).unwrap();
    editor.insert_text("\n", TransactionOrigin::Typing).unwrap();
    editor.undo().unwrap();
    assert_eq!(editor.snapshot().text(), "a");
}

#[test]
fn explicit_typing_groups_still_require_adjacent_carets() {
    let mut editor = Editor::new("");
    for (text, group) in [("a", 7), ("b", 8)] {
        let start = editor.snapshot().byte_len();
        editor
            .apply(
                Transaction::new(&editor.snapshot())
                    .replace(ByteOffset(start)..ByteOffset(start), text)
                    .with_selection(Selection::caret(ByteOffset(start + text.len())))
                    .with_origin(TransactionOrigin::Typing)
                    .with_group(group),
            )
            .unwrap();
    }
    editor.undo().unwrap();
    assert_eq!(editor.snapshot().text(), "a");
}

#[test]
fn mapping_respects_insert_affinity_and_replacement_length() {
    let mut editor = Editor::new("abéde");
    let result = editor
        .apply(
            Transaction::new(&editor.snapshot())
                .replace(ByteOffset(0)..ByteOffset(1), "XX")
                .replace(ByteOffset(4)..ByteOffset(5), "Z"),
        )
        .unwrap();
    assert_eq!(
        result
            .change_map
            .map_offset(ByteOffset(0), Affinity::Before)
            .unwrap(),
        ByteOffset(0)
    );
    assert_eq!(
        result
            .change_map
            .map_offset(ByteOffset(0), Affinity::After)
            .unwrap(),
        ByteOffset(2)
    );
    assert_eq!(
        result
            .change_map
            .map_offset(ByteOffset(2), Affinity::After)
            .unwrap(),
        ByteOffset(3)
    );
    assert_eq!(
        result
            .change_map
            .map_offset(ByteOffset(6), Affinity::After)
            .unwrap(),
        ByteOffset(7)
    );
}

#[test]
fn grapheme_commands_remove_combining_sequences_emoji_and_crlf_as_units() {
    for grapheme in ["e\u{301}", "👩🏽‍💻", "\r\n", "🇨🇦"] {
        let mut editor = Editor::new(&format!("a{grapheme}b"));
        editor
            .set_selection(Selection::caret(ByteOffset(1)))
            .unwrap();
        editor.move_grapheme(true, false).unwrap();
        assert_eq!(editor.selection().head, ByteOffset(1 + grapheme.len()));
        editor.delete_backward().unwrap().unwrap();
        assert_eq!(editor.snapshot().text(), "ab");
        editor.undo().unwrap();
        editor
            .set_selection(Selection::caret(ByteOffset(1)))
            .unwrap();
        editor.delete_forward().unwrap().unwrap();
        assert_eq!(editor.snapshot().text(), "ab");
    }
}

#[test]
fn explicit_scalar_conversions_refuse_mid_character_bytes() {
    let snapshot = Editor::new("aé中").snapshot();
    for scalar in 0..=3 {
        let offset = snapshot.scalar_to_byte(ScalarIndex(scalar)).unwrap();
        assert_eq!(
            snapshot.byte_to_scalar(offset).unwrap(),
            ScalarIndex(scalar)
        );
    }
    assert!(snapshot.byte_to_scalar(ByteOffset(2)).is_err());
    assert!(snapshot.scalar_to_byte(ScalarIndex(4)).is_err());
    assert!(snapshot.slice(ByteOffset(2)..ByteOffset(3)).is_err());
    assert_eq!(snapshot.chunks().collect::<String>(), snapshot.text());
}

#[test]
fn bounded_subscriber_disconnects_on_lag_and_never_blocks_edits() {
    let mut editor = Editor::new("");
    let receiver = editor.subscribe();
    for _ in 0..65 {
        editor
            .insert_text("x", TransactionOrigin::Programmatic)
            .unwrap();
    }
    let events: Vec<_> = receiver.try_iter().collect();
    assert_eq!(events.len(), 64);
    assert_eq!(events[0].before_revision, Revision(0));
    assert_eq!(events[63].revision, Revision(64));
    assert!(matches!(
        receiver.try_recv(),
        Err(TryRecvError::Disconnected)
    ));
    assert_eq!(editor.snapshot().byte_len(), 65);
    let fresh = editor.subscribe();
    editor
        .insert_text("y", TransactionOrigin::Programmatic)
        .unwrap();
    assert_eq!(fresh.try_recv().unwrap().revision, Revision(66));
}

#[test]
fn noops_overlaps_and_reserved_history_origins_are_refused_atomically() {
    let mut editor = Editor::new("abc");
    assert!(matches!(
        editor.apply(Transaction::new(&editor.snapshot())),
        Err(EditorError::EmptyTransaction)
    ));
    assert!(matches!(
        editor
            .apply(Transaction::new(&editor.snapshot()).replace(ByteOffset(0)..ByteOffset(1), "a")),
        Err(EditorError::EmptyTransaction)
    ));
    assert!(matches!(
        editor.apply(
            Transaction::new(&editor.snapshot())
                .replace(ByteOffset(0)..ByteOffset(0), "a")
                .replace(ByteOffset(0)..ByteOffset(0), "b")
        ),
        Err(EditorError::OverlappingChanges)
    ));
    assert!(matches!(
        editor.apply(
            Transaction::new(&editor.snapshot())
                .replace(ByteOffset(0)..ByteOffset(1), "z")
                .with_origin(TransactionOrigin::Undo)
        ),
        Err(EditorError::ReservedOrigin)
    ));
    assert_eq!(editor.snapshot().text(), "abc");
    assert_eq!(editor.snapshot().revision(), Revision(0));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(160))]

    #[test]
    fn arbitrary_unicode_edits_match_a_string_model_and_fully_undo(
        initial in prop::collection::vec(any::<char>(), 0..40),
        operations in prop::collection::vec((any::<usize>(), any::<usize>(), prop::collection::vec(any::<char>(), 0..12)), 1..60),
    ) {
        let original: String = initial.into_iter().collect();
        let mut model = original.clone();
        let mut editor = Editor::new(&original);
        let mut states = vec![original.clone()];
        for (a, b, inserted) in operations {
            let a = boundary(&model, a);
            let b = boundary(&model, b);
            let range = a.min(b)..a.max(b);
            let inserted: String = inserted.into_iter().collect();
            let mut expected = model.clone();
            expected.replace_range(range.clone(), &inserted);
            let before = editor.snapshot();
            let result = editor.apply(Transaction::new(&before).replace(ByteOffset(range.start)..ByteOffset(range.end), inserted));
            if expected == model {
                prop_assert!(matches!(result, Err(EditorError::EmptyTransaction)));
                prop_assert_eq!(editor.snapshot().revision(), before.revision());
            } else {
                let result = result.unwrap();
                prop_assert_eq!(result.before_revision, before.revision());
                let mut event_model = model.clone();
                apply_event(&mut event_model, &result.changes);
                prop_assert_eq!(&event_model, &expected);
                model = expected;
                states.push(model.clone());
                prop_assert_eq!(editor.snapshot().text(), model.clone());
            }
        }
        let final_state = model;
        for expected in states.iter().rev().skip(1) {
            let previous_revision = editor.snapshot().revision();
            let result = editor.undo().unwrap().unwrap();
            prop_assert!(result.revision > previous_revision);
            prop_assert_eq!(editor.snapshot().text(), expected.clone());
        }
        prop_assert!(!editor.can_undo());
        prop_assert_eq!(editor.snapshot().text(), original);
        while editor.can_redo() { editor.redo().unwrap(); }
        prop_assert_eq!(editor.snapshot().text(), final_state);
    }

    #[test]
    fn disjoint_multi_changes_and_inverse_agree_with_a_string_model(
        source in prop::collection::vec(any::<char>(), 6..40),
        first in prop::collection::vec(any::<char>(), 0..12),
        second in prop::collection::vec(any::<char>(), 0..12),
    ) {
        let original: String = source.into_iter().collect();
        let boundaries: Vec<_> = original.char_indices().map(|(i,_)| i).chain([original.len()]).collect();
        let mid = boundaries.len() / 2;
        let left = boundaries[0]..boundaries[1];
        let right = boundaries[mid]..boundaries[mid + 1];
        let first: String = first.into_iter().collect();
        let second: String = second.into_iter().collect();
        let mut expected = original.clone();
        expected.replace_range(right.clone(), &second);
        expected.replace_range(left.clone(), &first);
        let mut editor = Editor::new(&original);
        let result = editor.apply(Transaction::new(&editor.snapshot())
            .replace(ByteOffset(right.start)..ByteOffset(right.end), second)
            .replace(ByteOffset(left.start)..ByteOffset(left.end), first));
        if expected == original {
            prop_assert!(matches!(result, Err(EditorError::EmptyTransaction)));
        } else {
            result.unwrap();
            prop_assert_eq!(editor.snapshot().text(), expected.clone());
            editor.undo().unwrap().unwrap();
            prop_assert_eq!(editor.snapshot().text(), original);
            editor.redo().unwrap().unwrap();
            prop_assert_eq!(editor.snapshot().text(), expected);
        }
    }

    #[test]
    fn random_edit_undo_redo_and_stale_submissions_follow_a_history_model(
        initial in prop::collection::vec(any::<char>(), 0..30),
        operations in prop::collection::vec(
            (any::<u8>(), any::<usize>(), any::<usize>(), prop::collection::vec(any::<char>(), 0..10)),
            1..100,
        ),
    ) {
        let original: String = initial.into_iter().collect();
        let mut editor = Editor::new(&original);
        let stale = editor.snapshot();
        // Establish a stale base in every case, including generated sequences
        // consisting only of undo, redo, or rejected submissions.
        editor.insert_text("✓", TransactionOrigin::Programmatic).unwrap();
        let mut states = vec![original, editor.snapshot().text()];
        let mut cursor = 1;
        let mut revision = 1_u64;
        let events = editor.subscribe();

        for (action, a, b, inserted) in operations {
            let before = editor.snapshot();
            let selection_before = editor.selection();
            let before_text = states[cursor].clone();
            let event = match action % 5 {
                0 | 1 => {
                    let a = boundary(&before_text, a);
                    let b = boundary(&before_text, b);
                    let range = a.min(b)..a.max(b);
                    let inserted: String = inserted.into_iter().collect();
                    let mut expected = before_text.clone();
                    expected.replace_range(range.clone(), &inserted);
                    let result = editor.apply(Transaction::new(&before)
                        .replace(ByteOffset(range.start)..ByteOffset(range.end), inserted));
                    if expected == before_text {
                        prop_assert_eq!(result.unwrap_err(), EditorError::EmptyTransaction);
                        None
                    } else {
                        states.truncate(cursor + 1);
                        states.push(expected);
                        cursor += 1;
                        // New edits must discard a previously available redo branch.
                        prop_assert!(!editor.can_redo());
                        Some(result.unwrap())
                    }
                }
                2 => {
                    let result = editor.undo().unwrap();
                    if cursor == 0 {
                        prop_assert!(result.is_none());
                    } else {
                        cursor -= 1;
                        prop_assert!(result.is_some());
                    }
                    result
                }
                3 => {
                    let result = editor.redo().unwrap();
                    if cursor + 1 == states.len() {
                        prop_assert!(result.is_none());
                    } else {
                        cursor += 1;
                        prop_assert!(result.is_some());
                    }
                    result
                }
                _ => {
                    let rejected = Transaction::new(&stale)
                        .replace(ByteOffset(0)..ByteOffset(0), "stale submission");
                    prop_assert_eq!(editor.apply(rejected).unwrap_err(),
                        EditorError::StaleRevision { expected: Revision(0), actual: Revision(revision) });
                    prop_assert_eq!(editor.selection(), selection_before);
                    None
                }
            };

            if let Some(event) = event {
                revision += 1;
                prop_assert_eq!(event.before_revision, before.revision());
                prop_assert_eq!(event.revision, Revision(revision));
                let mut replay = before_text.clone();
                apply_event(&mut replay, &event.changes);
                prop_assert_eq!(&replay, &states[cursor]);
                let published = events.try_recv().unwrap();
                prop_assert_eq!(published.revision, event.revision);
                prop_assert_eq!(published.snapshot.text(), states[cursor].clone());
            }
            prop_assert!(matches!(events.try_recv(), Err(TryRecvError::Empty)));
            prop_assert_eq!(editor.snapshot().revision(), Revision(revision));
            prop_assert_eq!(editor.snapshot().text(), states[cursor].clone());
            prop_assert_eq!(editor.can_undo(), cursor > 0);
            prop_assert_eq!(editor.can_redo(), cursor + 1 < states.len());
            // Holding old snapshots must not mutate them during any branch.
            prop_assert_eq!(before.text(), before_text);
        }
    }
}
