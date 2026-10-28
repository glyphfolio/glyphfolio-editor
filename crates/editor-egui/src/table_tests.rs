use super::*;
use egui::{PointerButton, pos2};
fn frame(
    ctx: &egui::Context,
    editor: &mut Editor,
    view: &mut NativeViewState,
    events: Vec<Event>,
) -> Rect {
    let mut bounds = Rect::NOTHING;
    let mut output = ctx.run_ui(
        egui::RawInput {
            events,
            focused: true,
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(640.0, 480.0))),
            ..Default::default()
        },
        |ui| {
            bounds = ui
                .add(EditorWidget::new(editor, view).id(Id::unique("native-table-interaction")))
                .rect;
        },
    );
    output.textures_delta.clear();
    bounds
}
#[test]
fn clicking_and_editing_a_real_cell_targets_its_source_and_undoes_once() {
    let source = "| Name | Value |\n| --- | ---: |\n| **bold** | y |\n";
    let ctx = egui::Context::default();
    let mut editor = Editor::new(source);
    let mut view = NativeViewState::default();
    view.configure(&config::EditorConfig::glyphfolio()).unwrap();
    view.request_focus();
    let bounds = frame(&ctx, &mut editor, &mut view, vec![]);
    let cell = view
        .last_surface
        .as_ref()
        .unwrap()
        .geometry
        .document
        .table_cells
        .iter()
        .find(|cell| cell.row == 1 && cell.column == 1)
        .unwrap();
    let point = pos2(
        bounds.left() + cell.bounds.x + cell.bounds.width - 14.0,
        bounds.top() + cell.bounds.y + 14.0,
    );
    frame(
        &ctx,
        &mut editor,
        &mut view,
        vec![
            Event::PointerMoved(point),
            Event::PointerButton {
                pos: point,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
            Event::PointerButton {
                pos: point,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    let table = glyphfolio_editor_core::MarkdownTable::at(
        &editor.snapshot().text(),
        editor.selection().head,
    )
    .unwrap();
    assert_eq!(table.cell_at(editor.selection().head), (1, 1));
    assert_eq!(editor.snapshot().text(), source);
    assert!(!editor.can_undo());
    frame(
        &ctx,
        &mut editor,
        &mut view,
        vec![
            Event::Key {
                key: Key::Home,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            },
            Event::Text("Z".into()),
        ],
    );
    assert_eq!(
        editor.snapshot().text(),
        "| Name | Value |\n| --- | ---: |\n| **bold** | Zy |\n"
    );
    assert!(view.last_error.is_none(), "{:?}", view.last_error);
    view.undo(&mut editor, &ctx).unwrap();
    assert_eq!(editor.snapshot().text(), source);
    assert!(!editor.can_undo());
}
#[test]
fn cursor_changes_inside_bold_keep_exact_text_and_cursor_geometry_stable() {
    let source = "before **bold** after";
    let ctx = egui::Context::default();
    let mut editor = Editor::new(source);
    let mut view = NativeViewState::default();
    view.configure(&config::EditorConfig::glyphfolio()).unwrap();
    view.request_focus();
    frame(&ctx, &mut editor, &mut view, vec![]);
    let first = view.last_surface.as_ref().unwrap().clone();
    assert_eq!(first.geometry.document.text, "before bold after");
    for byte in [9, 10, 11, 12, 13] {
        editor
            .set_selection(Selection::caret(ByteOffset(byte)))
            .unwrap();
        frame(&ctx, &mut editor, &mut view, vec![]);
        let surface = view.last_surface.as_ref().unwrap();
        assert_eq!(surface.geometry.document.text, "before bold after");
        assert_eq!(
            surface.geometry.caret_with_affinity(byte, Affinity::After),
            first.geometry.caret_with_affinity(byte, Affinity::After)
        );
        assert_eq!(surface.metrics.shaped_blocks, 0);
    }
    assert_eq!(editor.snapshot().text(), source);
    assert!(!editor.can_undo());
}
