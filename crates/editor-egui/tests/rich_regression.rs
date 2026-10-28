//! Headless event-route regressions for the experimental Rich surface.
//! These do not qualify a native screen reader or native platform transport.

use egui::{Event, Id, Key, Modifiers, Pos2, Rect, vec2};
use glyphfolio_editor_core::{ByteOffset, EditCommand, Editor, PresentationMode, Selection};
use glyphfolio_editor_egui::{EditorWidget, NativeViewState};

fn frame(
    context: &egui::Context,
    editor: &mut Editor,
    state: &mut NativeViewState,
    events: Vec<Event>,
) {
    let mut output = context.run_ui(
        egui::RawInput {
            events,
            focused: true,
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(640.0, 480.0))),
            ..Default::default()
        },
        |ui| {
            ui.add(EditorWidget::new(editor, state).id(Id::unique("rich-regression")));
        },
    );
    output.textures_delta.clear();
}

fn rich(source: &str) -> (egui::Context, Editor, NativeViewState) {
    let context = egui::Context::default();
    context.enable_accesskit();
    let mut editor = Editor::new(source);
    editor
        .set_selection(Selection::caret(ByteOffset(source.len())))
        .unwrap();
    let mut state = NativeViewState::default();
    state.enable_experimental_rich(true);
    state.request_presentation(PresentationMode::Rich);
    state.request_focus();
    frame(&context, &mut editor, &mut state, vec![]);
    assert_eq!(state.rendered_presentation(), PresentationMode::Rich);
    (context, editor, state)
}

fn set_accessible_selection(state: &NativeViewState, start: usize, end: usize) -> Event {
    Event::AccessKitActionRequest(accesskit::ActionRequest {
        action: accesskit::Action::SetTextSelection,
        target_tree: accesskit::TreeId::ROOT,
        target_node: Id::unique("rich-regression").accesskit_id(),
        data: Some(accesskit::ActionData::SetTextSelection(
            accesskit::TextSelection {
                anchor: state.accessibility.position(ByteOffset(start)).unwrap(),
                focus: state.accessibility.position(ByteOffset(end)).unwrap(),
            },
        )),
    })
}

#[test]
fn folded_accessible_caret_stays_collapsed_before_typing() {
    let source = "**bold**\n\nother";
    let (context, mut editor, mut state) = rich(source);
    // First block displays "bold". Its display-zero boundary corresponds to
    // source zero before the opener and source two after the hidden opener.
    let select = set_accessible_selection(&state, 0, 0);
    frame(&context, &mut editor, &mut state, vec![select]);
    assert!(
        editor.selection().is_collapsed(),
        "{:?}",
        editor.selection()
    );
    frame(
        &context,
        &mut editor,
        &mut state,
        vec![Event::Text("X".into())],
    );
    assert!(editor.snapshot().text().contains("**"));
    assert_eq!(editor.snapshot().text().matches("**").count(), 2);
    editor.undo().unwrap();
    assert_eq!(editor.snapshot().text(), source);
}

#[test]
fn replacing_accessible_folded_content_preserves_markdown_delimiters() {
    let source = "**bold**\n\nother";
    let (context, mut editor, mut state) = rich(source);
    let select = set_accessible_selection(&state, 0, 4);
    frame(&context, &mut editor, &mut state, vec![select]);
    frame(
        &context,
        &mut editor,
        &mut state,
        vec![Event::Text("new".into())],
    );
    assert_eq!(editor.snapshot().text(), "**new**\n\nother");
}

#[test]
fn deleting_revealed_inline_content_retains_surrounding_source() {
    let source = "**bold**\n\nother";
    let (context, mut editor, mut state) = rich(source);
    editor
        .set_selection(Selection::caret(ByteOffset(6)))
        .unwrap();
    frame(&context, &mut editor, &mut state, vec![]);
    frame(
        &context,
        &mut editor,
        &mut state,
        vec![Event::Key {
            key: Key::Backspace,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
    );
    assert_eq!(editor.snapshot().text(), "**bol**\n\nother");
    editor.undo().unwrap();
    assert_eq!(editor.snapshot().text(), source);
}

#[test]
fn partial_transformed_accessible_selection_requires_reveal_before_editing() {
    let source = "a &fjlig; z\n\nother";
    let (context, mut editor, mut state) = rich(source);
    let selection_before = editor.selection();
    // &fjlig; renders two separately selectable display graphemes, "fj".
    // Replacing only "f" has no exact original-source slice. It must not turn
    // into a successful collapsed source caret before the entire entity.
    let select = set_accessible_selection(&state, 2, 3);
    frame(&context, &mut editor, &mut state, vec![select]);
    assert!(
        state.last_error.is_some(),
        "partial transform was accepted as {:?}",
        editor.selection()
    );
    assert_eq!(editor.selection(), selection_before);
    assert_eq!(editor.snapshot().text(), source);
}

#[test]
fn formatting_does_not_strip_commonmark_literal_delimiter_characters() {
    for (source, anchor, head, command) in [
        ("a_b_c", 2, 3, EditCommand::ToggleEmphasis),
        ("** bold **", 0, 10, EditCommand::ToggleStrong),
    ] {
        let (context, mut editor, mut state) = rich(source);
        editor
            .set_selection(Selection {
                anchor: ByteOffset(anchor),
                head: ByteOffset(head),
                ..Selection::default()
            })
            .unwrap();
        state
            .execute_command(&mut editor, command, &context)
            .unwrap();
        let after = editor.snapshot().text();
        assert!(
            after.matches('_').count() >= source.matches('_').count()
                && after.matches('*').count() >= source.matches('*').count(),
            "formatting removed CommonMark literal source characters: {source:?} -> {after:?}"
        );
    }
}

#[test]
fn moving_the_caret_through_formatting_never_reveals_delimiters_or_reflows() {
    let source = "# Title\n\n**bold** and *italic* and ~~done~~ and `code` and [link](https://example.org)\n\ntail";
    let (context, mut editor, mut state) = rich(source);
    let initial = state.render_metrics().unwrap().shaped_blocks;
    for at in [
        source.find("bold").unwrap() + 2,
        source.find("italic").unwrap() + 2,
        source.find("done").unwrap() + 1,
        source.find("code").unwrap() + 2,
    ] {
        editor
            .set_selection(Selection::caret(ByteOffset(at)))
            .unwrap();
        frame(&context, &mut editor, &mut state, vec![]);
        assert_eq!(
            state.render_metrics().unwrap().shaped_blocks,
            0,
            "caret change triggered reshaping after initial {initial}"
        );
        assert_eq!(editor.snapshot().text(), source);
        assert!(!editor.can_undo());
    }
}

#[test]
fn visual_deletion_preserves_partial_marks_and_removes_complete_wrappers() {
    for (source, a, b, expected) in [
        ("**bold** plain", 4, 10, "**bo**lain"),
        ("**bold** tail", 2, 6, " tail"),
        ("`code` tail", 1, 5, " tail"),
        ("**bold**", 5, 5, "**bod**"),
    ] {
        let (ctx, mut editor, mut state) = rich(source);
        editor
            .set_selection(Selection {
                anchor: ByteOffset(a),
                head: ByteOffset(b),
                ..Selection::default()
            })
            .unwrap();
        frame(&ctx, &mut editor, &mut state, vec![]);
        frame(
            &ctx,
            &mut editor,
            &mut state,
            vec![Event::Key {
                key: Key::Backspace,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
        );
        assert_eq!(editor.snapshot().text(), expected, "{:?}", state.last_error);
        editor.undo().unwrap();
        assert_eq!(editor.snapshot().text(), source);
    }
}

#[test]
fn collapsed_formatting_does_not_insert_empty_syntax_and_typing_stays_formatted() {
    let (ctx, mut editor, mut state) = rich("");
    state
        .execute_command(&mut editor, EditCommand::ToggleStrong, &ctx)
        .unwrap();
    assert_eq!(editor.snapshot().text(), "");
    assert!(!editor.can_undo());
    assert!(state.command_active(&editor, EditCommand::ToggleStrong));
    for text in ["b", "o", "ld", " ", "next"] {
        frame(
            &ctx,
            &mut editor,
            &mut state,
            vec![Event::Text(text.into())],
        );
    }
    assert_eq!(editor.snapshot().text(), "**bold** **next**");
    frame(&ctx, &mut editor, &mut state, vec![Event::Text("!".into())]);
    assert_eq!(editor.snapshot().text(), "**bold** **next!**");
    assert!(state.last_error.is_none(), "{:?}", state.last_error);
}

#[test]
fn space_at_the_end_of_an_existing_mark_does_not_expose_broken_delimiters() {
    let source = "**bold**";
    let (ctx, mut editor, mut state) = rich(source);
    editor
        .set_selection(Selection::caret(ByteOffset(6)))
        .unwrap();
    frame(&ctx, &mut editor, &mut state, vec![]);
    frame(&ctx, &mut editor, &mut state, vec![Event::Text(" ".into())]);
    assert_eq!(editor.snapshot().text(), "**bold** ");
    frame(
        &ctx,
        &mut editor,
        &mut state,
        vec![Event::Text("word".into())],
    );
    assert_eq!(editor.snapshot().text(), "**bold** **word**");
}

#[test]
fn enter_creates_real_paragraphs_with_balanced_inline_marks_and_one_undo_step() {
    for (source, at, expected) in [
        ("hello world", 5, "hello\n\n world"),
        ("**bold**", 4, "**bo**\n\n**ld**"),
        ("**bold**", 6, "**bold**\n\n"),
        ("**bold**", 2, "\n\n**bold**"),
        ("- **bold**", 6, "- **bo**\n- **ld**"),
    ] {
        let (ctx, mut editor, mut state) = rich(source);
        editor
            .set_selection(Selection::caret(ByteOffset(at)))
            .unwrap();
        state.begin_native_input(&editor).unwrap();
        frame(&ctx, &mut editor, &mut state, vec![]);
        frame(
            &ctx,
            &mut editor,
            &mut state,
            vec![Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
        );
        assert_eq!(editor.snapshot().text(), expected, "{:?}", state.last_error);
        state.undo(&mut editor, &ctx).unwrap();
        assert_eq!(editor.snapshot().text(), source);
        assert!(!editor.can_undo());
    }
}

#[test]
fn backspace_joins_text_paragraphs_and_merges_matching_marks_without_exposing_them() {
    for (source, at, expected) in [
        ("hello\n\nworld", 7, "helloworld"),
        ("**bo**\n\n**ld**", 10, "**bold**"),
        ("*left*\n\n*right*", 9, "*leftright*"),
        ("`left`\n\n`right`", 9, "`leftright`"),
    ] {
        let (ctx, mut editor, mut state) = rich(source);
        editor
            .set_selection(Selection::caret(ByteOffset(at)))
            .unwrap();
        state.begin_native_input(&editor).unwrap();
        frame(&ctx, &mut editor, &mut state, vec![]);
        frame(
            &ctx,
            &mut editor,
            &mut state,
            vec![Event::Key {
                key: Key::Backspace,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
        );
        assert_eq!(editor.snapshot().text(), expected, "{:?}", state.last_error);
        state.undo(&mut editor, &ctx).unwrap();
        assert_eq!(editor.snapshot().text(), source);
    }
}
