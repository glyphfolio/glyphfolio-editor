//! Public native-controller configuration policies, exercised through events.
//! These headless checks do not qualify OS input or screen-reader transport.

use std::sync::{Arc, Mutex};

use egui::{Event, Id, Key, Modifiers, Pos2, Rect, vec2};
use glyphfolio_editor_core::{ByteOffset, EditCommand, Editor, PresentationMode, Selection};
use glyphfolio_editor_egui::{
    EditorWidget, NativeViewState, clipboard::ClipboardService, config::EditorConfig,
};

fn frame(
    context: &egui::Context,
    editor: &mut Editor,
    state: &mut NativeViewState,
    events: Vec<Event>,
) -> egui::FullOutput {
    let mut output = context.run_ui(
        egui::RawInput {
            events,
            focused: true,
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(640.0, 480.0))),
            ..Default::default()
        },
        |ui| {
            ui.add(EditorWidget::new(editor, state).id(Id::unique("configured-input")));
        },
    );
    // Headless tests have no GPU integration to consume texture ownership.
    output.textures_delta.clear();
    output
}

fn focused(
    source: &str,
    selection: Selection,
    config: &EditorConfig,
) -> (egui::Context, Editor, NativeViewState) {
    let context = egui::Context::default();
    let mut editor = Editor::new(source);
    editor.set_selection(selection).unwrap();
    let mut view = NativeViewState::default();
    view.configure(config).unwrap();
    view.request_focus();
    frame(&context, &mut editor, &mut view, vec![]);
    assert!(context.memory(|memory| memory.has_focus(Id::unique("configured-input"))));
    (context, editor, view)
}

fn select(anchor: usize, head: usize) -> Selection {
    Selection {
        anchor: ByteOffset(anchor),
        head: ByteOffset(head),
        ..Default::default()
    }
}

fn command(key: Key) -> Event {
    Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers: Modifiers {
            command: true,
            mac_cmd: true,
            ..Default::default()
        },
    }
}

#[test]
fn a_hidden_toolbar_preserves_keyboard_formatting_and_shared_undo() {
    let mut config = EditorConfig::default();
    config.toolbar.visible = false;
    let (context, mut editor, mut view) = focused("word", select(0, 4), &config);
    frame(&context, &mut editor, &mut view, vec![command(Key::B)]);
    assert_eq!(editor.snapshot().text(), "**word**");
    assert!(view.undo(&mut editor, &context).unwrap());
    assert_eq!(editor.snapshot().text(), "word");
    assert_eq!(editor.selection(), select(0, 4));
    assert!(!editor.can_undo());
}

#[test]
fn disabled_bold_blocks_keyboard_and_host_tools_without_touching_source_or_history() {
    let mut config = EditorConfig::default();
    config.features.bold = false;
    let (context, mut editor, mut view) = focused("word", select(0, 4), &config);
    let initial = editor.snapshot();
    frame(&context, &mut editor, &mut view, vec![command(Key::B)]);
    assert!(
        view.execute_command(&mut editor, EditCommand::ToggleStrong, &context)
            .is_err()
    );
    assert_eq!(editor.snapshot().text(), initial.text());
    assert_eq!(editor.snapshot().revision(), initial.revision());
    assert_eq!(editor.selection(), select(0, 4));
    assert!(!editor.can_undo());
    assert!(!editor.can_redo());

    // The core remains an unrestricted host API, not a content sanitizer.
    assert!(editor.execute(EditCommand::ToggleStrong).unwrap().is_some());
    assert_eq!(editor.snapshot().text(), "**word**");
    editor.undo().unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), "word");
}

#[test]
fn temporarily_disabling_history_retains_the_stack_and_restores_both_entry_points() {
    let mut config = EditorConfig::default();
    let (context, mut editor, mut view) = focused("word", select(4, 4), &config);
    frame(
        &context,
        &mut editor,
        &mut view,
        vec![Event::Text("!".into())],
    );
    assert_eq!(editor.snapshot().text(), "word!");
    assert!(editor.can_undo());
    let after_typing = editor.snapshot();

    config.features.history = false;
    view.configure(&config).unwrap();
    frame(&context, &mut editor, &mut view, vec![command(Key::Z)]);
    assert!(view.undo(&mut editor, &context).is_err());
    assert!(view.redo(&mut editor, &context).is_err());
    assert_eq!(editor.snapshot().text(), after_typing.text());
    assert_eq!(editor.snapshot().revision(), after_typing.revision());
    assert!(editor.can_undo());
    assert!(!editor.can_redo());

    config.features.history = true;
    view.configure(&config).unwrap();
    assert!(view.undo(&mut editor, &context).unwrap());
    assert_eq!(editor.snapshot().text(), "word");
    assert!(view.redo(&mut editor, &context).unwrap());
    assert_eq!(editor.snapshot().text(), "word!");
    frame(&context, &mut editor, &mut view, vec![command(Key::Z)]);
    assert_eq!(editor.snapshot().text(), "word");
    assert!(!editor.can_undo());
}

#[derive(Default)]
struct ClipboardCalls {
    reads: usize,
    writes: usize,
}

struct ObservedClipboard(Arc<Mutex<ClipboardCalls>>);

impl ClipboardService for ObservedClipboard {
    fn write_text(&mut self, _text: &str) -> Result<(), String> {
        self.0.lock().unwrap().writes += 1;
        Ok(())
    }

    fn read_text(&mut self) -> Result<String, String> {
        self.0.lock().unwrap().reads += 1;
        Ok("from clipboard".into())
    }
}

#[test]
fn disabled_clipboard_never_calls_transport_or_applies_paste_and_cut() {
    let mut config = EditorConfig::default();
    config.features.clipboard = false;
    let (context, mut editor, mut view) = focused("selected", select(0, 8), &config);
    let calls = Arc::new(Mutex::new(ClipboardCalls::default()));
    view.set_clipboard(Box::new(ObservedClipboard(Arc::clone(&calls))));
    let initial = editor.snapshot();
    let output = frame(
        &context,
        &mut editor,
        &mut view,
        vec![Event::Paste("replacement".into()), Event::Copy, Event::Cut],
    );
    assert_eq!(calls.lock().unwrap().reads, 0);
    assert_eq!(calls.lock().unwrap().writes, 0);
    assert_eq!(editor.snapshot().text(), initial.text());
    assert_eq!(editor.snapshot().revision(), initial.revision());
    assert_eq!(editor.selection(), select(0, 8));
    assert!(!editor.can_undo());
    assert!(output.platform_output.commands.is_empty());
}

#[test]
fn configuring_during_ime_preserves_provisional_source_and_defers_rich_until_commit() {
    let original = "left TARGET right";
    let (context, mut editor, mut view) =
        focused(original, select(5, 11), &EditorConfig::default());
    frame(
        &context,
        &mut editor,
        &mut view,
        vec![Event::Ime(egui::ImeEvent::Preedit {
            text: "にほん".into(),
            active_range_chars: Some(0..3),
        })],
    );
    assert!(view.is_composing());
    assert!(!view.can_close());
    assert_eq!(editor.snapshot().text(), original);
    assert!(!editor.can_undo());

    let mut rich = EditorConfig::glyphfolio();
    rich.features.source = false;
    view.configure(&rich).unwrap();
    frame(&context, &mut editor, &mut view, vec![]);
    assert_eq!(view.configuration(), Some(&rich));
    assert!(view.is_composing());
    assert!(!view.can_close());
    assert_eq!(view.presentation(), PresentationMode::Source);
    assert_eq!(editor.snapshot().text(), original);
    assert_eq!(editor.selection(), select(5, 11));
    assert!(!editor.can_undo());

    frame(
        &context,
        &mut editor,
        &mut view,
        vec![Event::Ime(egui::ImeEvent::Commit("日本".into()))],
    );
    assert_eq!(editor.snapshot().text(), "left 日本 right");
    assert!(!view.is_composing());
    assert!(view.can_close());
    assert_eq!(view.presentation(), PresentationMode::Rich);
    assert_eq!(view.rendered_presentation(), PresentationMode::Rich);
    editor.undo().unwrap().unwrap();
    assert_eq!(editor.snapshot().text(), original);
    assert_eq!(editor.selection(), select(5, 11));
    assert!(!editor.can_undo());
}

#[test]
fn invalid_configuration_keeps_the_last_settings_document_and_selection() {
    let config = EditorConfig::default();
    let (context, mut editor, mut view) = focused("word", select(1, 3), &config);
    let initial = editor.snapshot();
    let mut invalid = EditorConfig::glyphfolio();
    invalid.document.line_height = 0.0;
    assert!(view.configure(&invalid).is_err());
    assert_eq!(view.configuration(), Some(&config));
    assert!(!view.experimental_rich_enabled());
    assert_eq!(view.presentation(), PresentationMode::Source);
    frame(&context, &mut editor, &mut view, vec![]);
    assert_eq!(editor.snapshot().text(), initial.text());
    assert_eq!(editor.snapshot().revision(), initial.revision());
    assert_eq!(editor.selection(), select(1, 3));
    assert!(!editor.can_undo());
}

struct HtmlClipboard;
impl ClipboardService for HtmlClipboard {
    fn write_text(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn read_text(&mut self) -> Result<String, String> {
        Ok("Title\nhello bold".into())
    }
    fn read_html(&mut self) -> Result<Option<String>, String> {
        Ok(Some("<h2>Title</h2><p>hello <b>bold</b></p>".into()))
    }
}
#[test]
fn rich_html_paste_is_atomic_and_source_paste_stays_verbatim() {
    for (mode, expected) in [
        (PresentationMode::Rich, "## Title\n\nhello **bold**"),
        (PresentationMode::Source, "Title\nhello bold"),
    ] {
        let config = EditorConfig::glyphfolio();
        let (ctx, mut editor, mut view) = focused("", Selection::default(), &config);
        view.request_presentation(mode);
        view.set_clipboard(Box::new(HtmlClipboard));
        frame(
            &ctx,
            &mut editor,
            &mut view,
            vec![Event::Paste("Title\nhello bold".into())],
        );
        assert_eq!(editor.snapshot().text(), expected);
        editor.undo().unwrap();
        assert_eq!(editor.snapshot().text(), "");
        assert!(!editor.can_undo());
    }
}

struct TestAssets;
impl glyphfolio_editor_egui::assets::AssetService for TestAssets {
    fn resolve_image(
        &mut self,
        _: &str,
    ) -> Result<Option<glyphfolio_editor_egui::assets::AssetImage>, String> {
        Ok(Some(glyphfolio_editor_egui::assets::AssetImage {
            size: [2, 2],
            rgba: vec![255; 16],
        }))
    }
    fn import(
        &mut self,
        _: glyphfolio_editor_egui::assets::AssetImport,
    ) -> Result<glyphfolio_editor_egui::assets::ImportedAsset, String> {
        Ok(glyphfolio_editor_egui::assets::ImportedAsset {
            destination: "assets/test.png".into(),
            label: "image".into(),
            image: true,
        })
    }
}
#[test]
fn imported_image_is_source_backed_drawn_and_undoable() {
    use glyphfolio_editor_egui::assets::AssetImport;
    let (ctx, mut editor, mut view) =
        focused("", Selection::default(), &EditorConfig::glyphfolio());
    view.set_asset_service(Box::new(TestAssets));
    view.import_asset(
        &mut editor,
        AssetImport::Bytes {
            name: "test.png".into(),
            bytes: Arc::from([0u8]),
        },
    )
    .unwrap();
    assert_eq!(editor.snapshot().text(), "![image](<assets/test.png>)");
    let output = frame(&ctx, &mut editor, &mut view, vec![]);
    assert!(!output.shapes.is_empty());
    assert!(view.last_error.is_none(), "{:?}", view.last_error);
    editor.undo().unwrap();
    assert_eq!(editor.snapshot().text(), "");
    let invalid = glyphfolio_editor_egui::assets::AssetImage {
        size: [usize::MAX, 2],
        rgba: vec![],
    };
    assert!(invalid.validate().is_err());
}

struct FileClipboard;
impl ClipboardService for FileClipboard {
    fn write_text(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn read_text(&mut self) -> Result<String, String> {
        Err("Files only".into())
    }
    fn read_files(&mut self) -> Result<Vec<std::path::PathBuf>, String> {
        Ok(vec!["one.png".into(), "two.png".into()])
    }
}
#[test]
fn file_clipboard_paste_is_one_history_step_and_respects_feature_policy() {
    let (ctx, mut editor, mut view) =
        focused("", Selection::default(), &EditorConfig::glyphfolio());
    view.set_asset_service(Box::new(TestAssets));
    view.set_clipboard(Box::new(FileClipboard));
    view.paste_clipboard(&mut editor, &ctx).unwrap();
    assert_eq!(editor.snapshot().text().matches("![image]").count(), 2);
    view.undo(&mut editor, &ctx).unwrap();
    assert_eq!(editor.snapshot().text(), "");
    assert!(!editor.can_undo());
    let mut config = EditorConfig::glyphfolio();
    config.features.attachments = false;
    view.configure(&config).unwrap();
    assert!(view.paste_clipboard(&mut editor, &ctx).is_err());
    assert_eq!(editor.snapshot().text(), "");
}

#[test]
fn rich_table_typing_preserves_cell_boundaries_and_tab_selects_next_cell() {
    let source = "| A | B |\n| --- | --- |\n| x | y |\n";
    let at = source.find("x |").unwrap();
    let (ctx, mut editor, mut view) =
        focused(source, select(at, at + 1), &EditorConfig::glyphfolio());
    frame(
        &ctx,
        &mut editor,
        &mut view,
        vec![Event::Text("a|b".into())],
    );
    assert!(editor.snapshot().text().contains("| a\\|b | y |"));
    view.undo(&mut editor, &ctx).unwrap();
    assert_eq!(editor.snapshot().text(), source);
    frame(
        &ctx,
        &mut editor,
        &mut view,
        vec![Event::Key {
            key: Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
    );
    assert_eq!(
        editor.snapshot().slice(editor.selection().range()).unwrap(),
        "y"
    );
    assert_eq!(view.rendered_presentation(), PresentationMode::Rich);
}

#[test]
fn native_image_paste_event_uses_host_import_and_shared_history() {
    let (ctx, mut editor, mut view) =
        focused("", Selection::default(), &EditorConfig::glyphfolio());
    view.set_asset_service(Box::new(TestAssets));
    let image = egui::ColorImage::filled([2, 2], egui::Color32::RED);
    frame(
        &ctx,
        &mut editor,
        &mut view,
        vec![Event::PasteImage(Arc::new(image))],
    );
    assert_eq!(editor.snapshot().text(), "![image](<assets/test.png>)");
    assert!(view.last_error.is_none(), "{:?}", view.last_error);
    view.undo(&mut editor, &ctx).unwrap();
    assert_eq!(editor.snapshot().text(), "");
}

#[test]
fn pending_rich_marks_apply_to_ime_commit_once_and_undo_atomically() {
    let (ctx, mut editor, mut view) =
        focused("", Selection::default(), &EditorConfig::glyphfolio());
    view.execute_command(&mut editor, EditCommand::ToggleStrong, &ctx)
        .unwrap();
    frame(&ctx, &mut editor, &mut view, vec![]);
    frame(
        &ctx,
        &mut editor,
        &mut view,
        vec![Event::Ime(egui::ImeEvent::Preedit {
            text: "にほん".into(),
            active_range_chars: None,
        })],
    );
    assert_eq!(editor.snapshot().text(), "");
    frame(
        &ctx,
        &mut editor,
        &mut view,
        vec![
            Event::Ime(egui::ImeEvent::Commit("日本".into())),
            Event::Text("日本".into()),
        ],
    );
    assert_eq!(editor.snapshot().text(), "**日本**");
    assert!(view.last_error.is_none(), "{:?}", view.last_error);
    view.undo(&mut editor, &ctx).unwrap();
    assert_eq!(editor.snapshot().text(), "");
    assert!(!editor.can_undo());
    view.redo(&mut editor, &ctx).unwrap();
    assert_eq!(editor.snapshot().text(), "**日本**");
    assert_eq!(editor.selection().head, ByteOffset(8));
}

#[test]
fn shift_enter_adds_a_cell_line_break_and_boundary_delete_keeps_table_syntax() {
    let source = "| A | B |\n| --- | --- |\n| x | y |\n";
    let at = source.find('x').unwrap() + 1;
    let (ctx, mut editor, mut view) = focused(source, select(at, at), &EditorConfig::glyphfolio());
    frame(
        &ctx,
        &mut editor,
        &mut view,
        vec![Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::NONE
            },
        }],
    );
    assert!(editor.snapshot().text().contains("x<br>"));
    frame(
        &ctx,
        &mut editor,
        &mut view,
        vec![Event::Text("second".into())],
    );
    assert!(editor.snapshot().text().contains("x<br>second"));
    let table = glyphfolio_editor_core::MarkdownTable::at(
        &editor.snapshot().text(),
        editor.selection().head,
    )
    .unwrap();
    assert_eq!(table.rows.len(), 2);
    let cell = table.rows[1].cells[1].clone();
    editor.set_selection(Selection::caret(cell.start)).unwrap();
    frame(&ctx, &mut editor, &mut view, vec![]);
    let before = editor.snapshot().text();
    frame(
        &ctx,
        &mut editor,
        &mut view,
        vec![Event::Key {
            key: Key::Backspace,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
    );
    assert_eq!(editor.snapshot().text(), before);
}

#[test]
fn arrow_down_leaves_the_last_table_row_and_typing_does_not_create_an_accidental_row() {
    let source = "| A | B |\n| --- | --- |\n| x | y |\n";
    let at = source.find('y').unwrap() + 1;
    let (ctx, mut editor, mut view) = focused(source, select(at, at), &EditorConfig::glyphfolio());
    frame(
        &ctx,
        &mut editor,
        &mut view,
        vec![Event::Key {
            key: Key::ArrowDown,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
    );
    frame(
        &ctx,
        &mut editor,
        &mut view,
        vec![Event::Text("After table".into())],
    );
    assert_eq!(editor.snapshot().text(), format!("{source}\nAfter table"));
    assert_eq!(
        glyphfolio_editor_core::MarkdownTable::at(&editor.snapshot().text(), ByteOffset(2))
            .unwrap()
            .rows
            .len(),
        2
    );
}
