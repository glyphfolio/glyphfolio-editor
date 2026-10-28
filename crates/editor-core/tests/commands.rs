use glyphfolio_editor_core::{
    Affinity, ByteOffset, EditCommand, Editor, HeadingLevel, Selection, TransactionOrigin,
};
use proptest::prelude::*;

fn select(editor: &mut Editor, anchor: usize, head: usize) {
    editor
        .set_selection(Selection {
            anchor: ByteOffset(anchor),
            head: ByteOffset(head),
            affinity: Affinity::After,
        })
        .unwrap();
}

fn command_at(text: &str, at: usize, command: EditCommand, expected: &str) {
    let mut editor = Editor::new(text);
    select(&mut editor, at, at);
    let original_selection = editor.selection();
    let before = editor.snapshot();
    editor
        .execute(command)
        .unwrap()
        .expect("fixture changes source");
    assert_eq!(editor.snapshot().text(), expected);
    let after = editor.snapshot();
    editor.undo().unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), text);
    assert_eq!(editor.selection(), original_selection);
    assert!(!editor.can_undo());
    assert_eq!(before.text(), text, "snapshots remain immutable");
    editor.redo().unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), after.text());
}

#[test]
fn inline_commands_expand_partial_graphemes_and_keep_reverse_selection() {
    let mut editor = Editor::new("before e\u{301} 日本 after");
    select(&mut editor, 10, 8); // e+combining acute includes a scalar-only endpoint.
    let initial = editor.snapshot();
    let selected = editor.selection();
    editor.execute(EditCommand::ToggleStrong).unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), "before **e\u{301}** 日本 after");
    assert!(editor.selection().anchor > editor.selection().head);
    assert_eq!(
        editor.snapshot().slice(editor.selection().range()).unwrap(),
        "e\u{301}"
    );
    editor.undo().unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), initial.text());
    assert_eq!(editor.selection(), selected);
}

#[test]
fn marks_preserve_nested_strong_emphasis_and_existing_marker_spelling() {
    for (source, command, expected) in [
        ("**bold**", EditCommand::ToggleEmphasis, "***bold***"),
        ("*italic*", EditCommand::ToggleStrong, "***italic***"),
        ("***both***", EditCommand::ToggleStrong, "*both*"),
        ("***both***", EditCommand::ToggleEmphasis, "**both**"),
        ("__bold__", EditCommand::ToggleStrong, "bold"),
        ("_italic_", EditCommand::ToggleEmphasis, "italic"),
    ] {
        let mut editor = Editor::new(source);
        select(&mut editor, 0, source.len());
        editor.execute(command).unwrap().unwrap();
        assert_eq!(editor.snapshot().text(), expected);
        editor.undo().unwrap().unwrap();
        assert_eq!(editor.snapshot().text(), source);
    }
    let mut editor = Editor::new("**content**");
    select(&mut editor, 2, 9);
    editor.execute(EditCommand::ToggleStrong).unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), "content");
    assert_eq!(editor.selection().range(), ByteOffset(0)..ByteOffset(7));
}

#[test]
fn literal_marks_are_preserved_when_they_are_not_a_commonmark_envelope() {
    for (source, anchor, head, command, expected) in [
        ("a_b_c", 2, 3, EditCommand::ToggleEmphasis, "a_*b*_c"),
        ("a__b__c", 3, 4, EditCommand::ToggleStrong, "a__**b**__c"),
        (
            "** bold **",
            0,
            10,
            EditCommand::ToggleStrong,
            "**** bold ****",
        ),
        (
            "a*\"foo\"*",
            2,
            7,
            EditCommand::ToggleEmphasis,
            "a**\"foo\"**",
        ),
        ("*$*alpha", 1, 2, EditCommand::ToggleEmphasis, "**$**alpha"),
        (
            "a**b****c",
            3,
            4,
            EditCommand::ToggleStrong,
            "a****b******c",
        ),
        (
            "**foo **bar baz**",
            0,
            17,
            EditCommand::ToggleStrong,
            "****foo **bar baz****",
        ),
        ("\\`a`", 2, 3, EditCommand::ToggleInlineCode, "\\``a``"),
    ] {
        let mut editor = Editor::new(source);
        select(&mut editor, anchor, head);
        let selected = editor.selection();
        editor.execute(command).unwrap().unwrap();
        assert_eq!(editor.snapshot().text(), expected, "source {source:?}");
        editor.undo().unwrap().unwrap();
        assert_eq!(editor.snapshot().text(), source);
        assert_eq!(editor.selection(), selected);
    }
}

#[test]
fn exact_unicode_flanking_preserves_intraword_marks_and_removes_real_envelopes() {
    for (source, anchor, head, expected) in [
        ("日_本_語", 4, 7, "日_*本*_語"),
        ("a*€*alpha", 2, 5, "a**€**alpha"),
        ("*\u{a0}word\u{a0}*", 0, 10, "**\u{a0}word\u{a0}**"),
        ("*€*", 1, 4, "€"),
        ("«_word_»", 3, 7, "«word»"),
        ("*e\u{301}*", 1, 4, "e\u{301}"),
    ] {
        let mut editor = Editor::new(source);
        select(&mut editor, anchor, head);
        editor
            .execute(EditCommand::ToggleEmphasis)
            .unwrap()
            .unwrap();
        assert_eq!(editor.snapshot().text(), expected, "source {source:?}");
        editor.undo().unwrap().unwrap();
        assert_eq!(editor.snapshot().text(), source);
    }
}

#[test]
fn multiline_marks_preserve_blank_lines_endings_and_unselected_source() {
    let source = "start\r\n  **one**  \r\n\r\n  two  \nend";
    let mut editor = Editor::new(source);
    let begin = source.find("  **").unwrap();
    let end = source.find("\nend").unwrap();
    select(&mut editor, end, begin);
    let events = editor.subscribe();
    editor.execute(EditCommand::ToggleStrong).unwrap().unwrap();
    assert_eq!(
        editor.snapshot().text(),
        "start\r\n  **one**  \r\n\r\n  **two**  \nend"
    );
    assert!(editor.selection().anchor > editor.selection().head);
    assert_eq!(events.try_iter().count(), 1);
    editor.undo().unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), source);
}

#[test]
fn whitespace_only_formatting_and_existing_heading_level_are_noops() {
    let mut editor = Editor::new(" \t \n");
    select(&mut editor, 0, 3);
    let before = editor.snapshot();
    let selected = editor.selection();
    let events = editor.subscribe();
    assert!(
        editor
            .execute(EditCommand::ToggleEmphasis)
            .unwrap()
            .is_none()
    );
    assert_eq!(editor.snapshot().revision(), before.revision());
    assert_eq!(editor.selection(), selected);
    assert!(events.try_recv().is_err());
    assert!(!editor.can_undo());
    let mut editor = Editor::new("## ready\r\n");
    assert!(
        editor
            .execute(EditCommand::SetHeading(Some(HeadingLevel::Two)))
            .unwrap()
            .is_none()
    );
    assert!(!editor.can_undo());
}

#[test]
fn collapsed_inline_formatting_places_caret_inside_an_explicit_source_pair() {
    for (command, expected, caret) in [
        (EditCommand::ToggleStrong, "a****b", 3),
        (EditCommand::ToggleEmphasis, "a**b", 2),
        (EditCommand::ToggleInlineCode, "a``b", 2),
    ] {
        let mut editor = Editor::new("ab");
        select(&mut editor, 1, 1);
        editor.execute(command).unwrap().unwrap();
        assert_eq!(editor.snapshot().text(), expected);
        assert_eq!(editor.selection().head, ByteOffset(caret));
        editor
            .insert_text("日本", TransactionOrigin::Typing)
            .unwrap();
        editor.undo().unwrap().unwrap();
        assert_eq!(
            editor.snapshot().text(),
            expected,
            "typing cannot join the formatting step"
        );
        editor.undo().unwrap().unwrap();
        assert_eq!(editor.snapshot().text(), "ab");
    }
}

#[test]
fn inline_code_chooses_safe_delimiters_and_padding_without_mutating_content() {
    for (source, expected) in [
        ("`a``", "``` `a`` ```"),
        (" x ", "`  x  `"),
        ("   ", "`   `"),
        ("日本\r\nnext", "`日本\r\nnext`"),
    ] {
        let mut editor = Editor::new(source);
        select(&mut editor, 0, source.len());
        editor
            .execute(EditCommand::ToggleInlineCode)
            .unwrap()
            .unwrap();
        assert_eq!(editor.snapshot().text(), expected);
        editor.undo().unwrap().unwrap();
        assert_eq!(editor.snapshot().text(), source);
        let mut wrapped = Editor::new(expected);
        select(&mut wrapped, 0, expected.len());
        wrapped
            .execute(EditCommand::ToggleInlineCode)
            .unwrap()
            .unwrap();
        assert_eq!(wrapped.snapshot().text(), source);
    }
    let mut editor = Editor::new("`a``b`");
    select(&mut editor, 0, 6);
    editor
        .execute(EditCommand::ToggleInlineCode)
        .unwrap()
        .unwrap();
    assert_eq!(
        editor.snapshot().text(),
        "a``b",
        "longer inner runs do not close a shorter code span"
    );
}

#[test]
fn headings_preserve_bom_containers_and_only_selected_physical_lines() {
    command_at(
        "\u{feff}> - title\r\nother\n",
        10,
        EditCommand::SetHeading(Some(HeadingLevel::Three)),
        "\u{feff}> - ### title\r\nother\n",
    );
    command_at(
        "### title ###  \r\nother",
        6,
        EditCommand::SetHeading(None),
        "title\r\nother",
    );
    command_at(
        "title\r\n=====\r\nother",
        2,
        EditCommand::SetHeading(Some(HeadingLevel::Two)),
        "## title\r\nother",
    );
    command_at("title\n-----", 8, EditCommand::SetHeading(None), "title");
    let mut editor = Editor::new("a\r\nb\r\nc");
    select(&mut editor, 0, 3);
    editor
        .execute(EditCommand::SetHeading(Some(HeadingLevel::One)))
        .unwrap()
        .unwrap();
    assert_eq!(
        editor.snapshot().text(),
        "# a\r\nb\r\nc",
        "end-at-next-line excludes that line"
    );
    let mut editor = Editor::new("- list\n---");
    editor
        .execute(EditCommand::SetHeading(Some(HeadingLevel::Two)))
        .unwrap()
        .unwrap();
    assert_eq!(
        editor.snapshot().text(),
        "- ## list\n---",
        "a neighboring thematic break is not a setext underline for a list"
    );
}

#[test]
fn list_and_quote_operations_preserve_nested_containers_and_exact_undo() {
    command_at(
        "> + item\r\nother",
        5,
        EditCommand::ToggleUnorderedList,
        "> item\r\nother",
    );
    command_at(
        ">   item\n",
        5,
        EditCommand::ToggleOrderedList,
        ">   1. item\n",
    );
    let mut editor = Editor::new("> - one\r\n> + two\r\noutside");
    select(&mut editor, 0, 18);
    let source = editor.snapshot().text();
    editor
        .execute(EditCommand::ToggleUnorderedList)
        .unwrap()
        .unwrap();
    assert_eq!(editor.snapshot().text(), "> one\r\n> two\r\noutside");
    editor.undo().unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), source);
    let mut editor = Editor::new("> one\nplain\n");
    select(&mut editor, 0, 11);
    editor
        .execute(EditCommand::ToggleBlockQuote)
        .unwrap()
        .unwrap();
    assert_eq!(editor.snapshot().text(), "> one\n> plain\n");
    editor
        .execute(EditCommand::ToggleBlockQuote)
        .unwrap()
        .unwrap();
    assert_eq!(editor.snapshot().text(), "one\nplain\n");
    command_at("> > nested", 6, EditCommand::ToggleBlockQuote, "> nested");
}

#[test]
fn indentation_is_an_atomic_source_edit_and_outdent_handles_tabs() {
    let mut editor = Editor::new("\u{feff}one\r\n\ttwo\nthree");
    select(&mut editor, 3, 13);
    let original = editor.snapshot();
    editor.execute(EditCommand::Indent).unwrap().unwrap();
    assert_eq!(
        editor.snapshot().text(),
        "\u{feff}    one\r\n    \ttwo\nthree"
    );
    editor.execute(EditCommand::Outdent).unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), original.text());
    editor.undo().unwrap().unwrap();
    editor.undo().unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), original.text());
    command_at("\tcode", 1, EditCommand::Backspace, "code");
    let mut editor = Editor::new("plain");
    assert!(editor.execute(EditCommand::Outdent).unwrap().is_none());
}

#[test]
fn enter_continues_lists_quotes_and_preserves_local_crlf() {
    command_at("- 日本", 8, EditCommand::Enter, "- 日本\n- ");
    command_at(
        "> 009) item\r\nnext",
        11,
        EditCommand::Enter,
        "> 009) item\r\n> 010) \r\nnext",
    );
    command_at("> text", 6, EditCommand::Enter, "> text\n> ");
    command_at("- abcdef", 5, EditCommand::Enter, "- abc\n- def");
    command_at("# title", 7, EditCommand::Enter, "# title\n");
    command_at("first\rsecond", 12, EditCommand::Enter, "first\rsecond\r");
    command_at(
        "    - literal code",
        18,
        EditCommand::Enter,
        "    - literal code\n    ",
    );
    command_at(
        "- parent\n    - child",
        20,
        EditCommand::Enter,
        "- parent\n    - child\n    - ",
    );
}

#[test]
fn enter_exits_empty_structural_markers_one_level_without_phantom_newlines() {
    command_at("- ", 2, EditCommand::Enter, "");
    command_at("> - ", 4, EditCommand::Enter, "> ");
    command_at("> > ", 4, EditCommand::Enter, "> ");
    command_at("- parent\n    - ", 15, EditCommand::Enter, "- parent\n");
}

#[test]
fn backspace_removes_only_the_current_structural_prefix_or_one_grapheme() {
    command_at("> - text", 4, EditCommand::Backspace, "> text");
    command_at("## text", 3, EditCommand::Backspace, "text");
    command_at("> > text", 4, EditCommand::Backspace, "> text");
    command_at("e\u{301}", 1, EditCommand::Backspace, "");
    command_at("one\r\ntwo", 5, EditCommand::Backspace, "onetwo");
}

#[test]
fn fenced_code_uses_a_longer_delimiter_and_does_not_continue_markdown_inside() {
    let source = "alpha\n```\nomega";
    let mut editor = Editor::new(source);
    select(&mut editor, 0, source.len());
    editor
        .execute(EditCommand::ToggleCodeFence)
        .unwrap()
        .unwrap();
    assert_eq!(editor.snapshot().text(), "````\nalpha\n```\nomega\n````");
    assert_eq!(
        editor.snapshot().slice(editor.selection().range()).unwrap(),
        source
    );
    editor
        .execute(EditCommand::ToggleCodeFence)
        .unwrap()
        .unwrap();
    assert_eq!(editor.snapshot().text(), source);
    command_at(
        "```rust\r\n- item\r\n```\r\n",
        15,
        EditCommand::Enter,
        "```rust\r\n- item\r\n\r\n```\r\n",
    );
    command_at(
        "```\n- item\n```",
        6,
        EditCommand::Backspace,
        "```\n-item\n```",
    );
    command_at("", 0, EditCommand::ToggleCodeFence, "```\n\n```");
}

#[test]
fn fencing_and_unfencing_preserve_eof_ending_policy_and_bom() {
    for source in ["text", "text\n", "text\r\n", "\u{feff}text", "\n"] {
        let mut editor = Editor::new(source);
        select(&mut editor, source.len(), source.len());
        editor
            .execute(EditCommand::ToggleCodeFence)
            .unwrap()
            .unwrap();
        editor
            .execute(EditCommand::ToggleCodeFence)
            .unwrap()
            .unwrap();
        assert_eq!(editor.snapshot().text(), source);
        editor.undo().unwrap().unwrap();
        editor.undo().unwrap().unwrap();
        assert_eq!(editor.snapshot().text(), source);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]
    #[test]
    fn arbitrary_commands_remain_atomic_and_fully_undo(text in "[a-zé日本😀\\n\\r\\t *#`>_-]{0,120}", operations in prop::collection::vec((0u8..14, any::<usize>(), any::<usize>()), 1..20)) {
        let mut editor = Editor::new(&text);
        let commands = [EditCommand::ToggleStrong, EditCommand::ToggleEmphasis, EditCommand::ToggleInlineCode, EditCommand::SetHeading(Some(HeadingLevel::Two)), EditCommand::SetHeading(None), EditCommand::ToggleUnorderedList, EditCommand::ToggleOrderedList, EditCommand::ToggleBlockQuote, EditCommand::ToggleCodeFence, EditCommand::Indent, EditCommand::Outdent, EditCommand::Enter, EditCommand::Backspace, EditCommand::SetHeading(Some(HeadingLevel::Six))];
        for (command, anchor, head) in operations {
            let source = editor.snapshot();
            let value = source.text();
            let boundaries: Vec<_> = value.char_indices().map(|(index, _)| index).chain([value.len()]).collect();
            select(&mut editor, boundaries[anchor % boundaries.len()], boundaries[head % boundaries.len()]);
            let selected = editor.selection();
            let result = editor.execute(commands[command as usize]);
            prop_assert!(result.is_ok(), "valid command rejected source {:?}: {:?}", value, result.err());
            match result {
                Ok(Some(event)) => {
                    let after = editor.snapshot();
                    prop_assert_eq!(event.before_revision, source.revision());
                    prop_assert!(after.is_char_boundary(editor.selection().anchor));
                    prop_assert!(after.is_char_boundary(editor.selection().head));
                    editor.undo().unwrap().unwrap();
                    prop_assert_eq!(editor.snapshot().text(), value.clone());
                    prop_assert_eq!(editor.selection(), selected);
                    editor.redo().unwrap().unwrap();
                    prop_assert_eq!(editor.snapshot().text(), after.text());
                }
                Ok(None) => { prop_assert_eq!(editor.snapshot().text(), value); prop_assert_eq!(editor.selection(), selected); }
                Err(_) => { prop_assert_eq!(editor.snapshot().text(), value); prop_assert_eq!(editor.snapshot().revision(), source.revision()); prop_assert_eq!(editor.selection(), selected); }
            }
        }
        while editor.undo().unwrap().is_some() {}
        prop_assert_eq!(editor.snapshot().text(), text);
    }
}
