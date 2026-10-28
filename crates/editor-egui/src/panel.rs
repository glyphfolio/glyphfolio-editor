//! A configured native editing panel for applications embedding this library.
//!
//! The panel borrows the host's original `Editor` and `NativeViewState`. Save
//! and diagnostic controls return intents; filesystem and host windows remain
//! the application's responsibility.

use egui::{Align, Id, Layout, Response, RichText, Ui, vec2};
use glyphfolio_editor_core::{DocumentId, Editor, Revision};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    EditorWidget, NativeViewState,
    chrome::{EditorAction, EditorToolbar, ThemePalette},
    config::{ConfigError, EditorConfig},
};

/// The editor surface response and host-owned actions requested this frame.
#[derive(Debug)]
pub struct EditorPanelResponse {
    pub response: Response,
    /// The host should save an immutable source snapshot using its own service.
    pub save_requested: bool,
    /// The host should toggle its diagnostic window or panel.
    pub diagnostics_requested: bool,
    /// The host should show its editor configuration controls.
    pub settings_requested: bool,
}

/// A drop-in configured toolbar and scrollable native editor surface.
///
/// Use a stable distinct `id` for each simultaneously visible editor panel.
/// Configuration changes never replace the borrowed document or history.
pub struct EditorPanel<'a> {
    editor: &'a mut Editor,
    view: &'a mut NativeViewState,
    config: &'a EditorConfig,
    id: Id,
    max_height: Option<f32>,
    with_gutters: bool,
    show_status: bool,
}

impl<'a> EditorPanel<'a> {
    pub fn new(
        editor: &'a mut Editor,
        view: &'a mut NativeViewState,
        config: &'a EditorConfig,
    ) -> Self {
        Self {
            editor,
            view,
            config,
            id: Id::unique("glyphfolio-editor-panel"),
            max_height: None,
            with_gutters: true,
            show_status: true,
        }
    }

    pub fn id(mut self, id: Id) -> Self {
        self.id = id;
        self
    }

    /// Maximum panel height, including toolbar and optional status line.
    /// Positive infinity uses the host's available height.
    pub fn max_height(mut self, height: f32) -> Self {
        self.max_height = Some(height);
        self
    }

    /// Disable outer gutters when the host has already aligned the editor.
    pub fn with_gutters(mut self, enabled: bool) -> Self {
        self.with_gutters = enabled;
        self
    }

    /// Hide the panel status line when the host already provides one.
    /// The configuration's `status.visible` is still honored when enabled.
    pub fn show_status(mut self, enabled: bool) -> Self {
        self.show_status = enabled;
        self
    }

    /// Validate configuration, dispatch enabled tools, and render the same editor.
    /// Invalid settings return before changing view or document state.
    pub fn show(self, ui: &mut Ui) -> Result<EditorPanelResponse, ConfigError> {
        self.config.validate()?;
        if self
            .max_height
            .is_some_and(|height| height.is_nan() || height <= 0.0)
        {
            return Err(ConfigError::Invalid {
                field: "panel.max_height".into(),
                reason: "must be a positive height".into(),
            });
        }
        if self.view.configuration() != Some(self.config) {
            self.view.configure(self.config)?;
        }
        let palette = ThemePalette::from_mode(ui.ctx(), self.config.appearance.theme);
        let available_width = ui.available_width().max(80.0);
        let gutter = if self.with_gutters {
            self.config
                .document
                .padding
                .min((available_width - 80.0).max(0.0) * 0.5)
        } else {
            0.0
        };
        let width = self
            .config
            .document
            .max_width
            .min((available_width - 2.0 * gutter).max(80.0));
        let height = self
            .max_height
            .unwrap_or_else(|| ui.available_height())
            .min(ui.available_height())
            .max(40.0);
        let status_visible = self.show_status && self.config.status.visible;
        let context = ui.ctx().clone();
        self.view.show_link_dialog(self.editor, &context);
        let response = ui.push_id(self.id, |ui| {
            ui.with_layout(Layout::top_down(Align::Center), |ui| {
                ui.allocate_ui_with_layout(
                    vec2(width, height),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.set_width(width);
                        palette.apply_to_ui(ui);
                        egui::Frame::new()
                            .fill(palette.editor_background)
                            .show(ui, |ui| {
                                let top = ui.cursor().top();
                                let mut pending = PendingActions::default();
                                let actions =
                                    EditorToolbar::show(ui, self.editor, self.view, self.config);
                                for action in actions {
                                    if let Err(error) = dispatch_action(
                                        action,
                                        self.editor,
                                        self.view,
                                        &context,
                                        &mut pending,
                                    ) {
                                        self.view.last_error = Some(error);
                                    }
                                }
                                if self.config.toolbar.visible {
                                    ui.add_space(20.0);
                                }
                                let used = ui.cursor().top() - top;
                                let footer = if status_visible { 26.0 } else { 0.0 };
                                let scroll_height = (height - used - footer).max(40.0);
                                let body = egui::ScrollArea::both()
                                    .id_salt(self.id.with("scroll"))
                                    .max_height(scroll_height)
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        ui.set_width(width);
                                        ui.add(
                                            EditorWidget::new(self.editor, self.view)
                                                .id(self.id.with("surface")),
                                        )
                                    });
                                if status_visible {
                                    ui.add_space(5.0);
                                    status_line(ui, self.id, self.editor, self.config, &palette);
                                }
                                EditorPanelResponse {
                                    response: body.inner,
                                    save_requested: pending.save_requested,
                                    diagnostics_requested: pending.diagnostics_requested,
                                    settings_requested: pending.settings_requested,
                                }
                            })
                            .inner
                    },
                )
                .inner
            })
            .inner
        });
        Ok(response.inner)
    }
}

#[derive(Default)]
struct PendingActions {
    settings_requested: bool,
    save_requested: bool,
    diagnostics_requested: bool,
}

fn dispatch_action(
    action: EditorAction,
    editor: &mut Editor,
    view: &mut NativeViewState,
    context: &egui::Context,
    pending: &mut PendingActions,
) -> Result<(), String> {
    match action {
        EditorAction::Command(command) => {
            view.execute_command(editor, command, context).map(|_| ())
        }
        EditorAction::Undo => view.undo(editor, context).map(|_| ()),
        EditorAction::Redo => view.redo(editor, context).map(|_| ()),
        EditorAction::Presentation(mode) => {
            view.request_presentation(mode);
            Ok(())
        }
        EditorAction::Paste => view.paste_clipboard(editor, context),
        EditorAction::EditLink => view.begin_link_edit(editor),
        EditorAction::Save => {
            if view.is_composing() {
                return Err("Finish or cancel composition before saving".into());
            }
            pending.save_requested = true;
            Ok(())
        }
        EditorAction::Settings => {
            pending.settings_requested = true;
            Ok(())
        }
        EditorAction::ToggleDiagnostics => {
            pending.diagnostics_requested = true;
            Ok(())
        }
        EditorAction::CancelComposition => {
            view.cancel_composition();
            context.memory_mut(|memory| memory.interrupt_ime());
            Ok(())
        }
        EditorAction::BeginFreshInput => {
            context.memory_mut(|memory| memory.interrupt_ime());
            view.begin_native_input(editor)?;
            view.request_focus();
            Ok(())
        }
    }
}

#[derive(Clone)]
struct WordCount {
    document: DocumentId,
    revision: Revision,
    words: usize,
}

fn status_line(
    ui: &mut Ui,
    id: Id,
    editor: &Editor,
    config: &EditorConfig,
    palette: &ThemePalette,
) {
    ui.horizontal(|ui| {
        if config.status.word_count {
            let snapshot = editor.snapshot();
            let key = id.with("source-word-count");
            let cached = ui.data_mut(|data| data.get_temp::<WordCount>(key));
            let count = cached
                .filter(|value| {
                    value.document == snapshot.document_id()
                        && value.revision == snapshot.revision()
                })
                .unwrap_or_else(|| {
                    let count = WordCount {
                        document: snapshot.document_id(),
                        revision: snapshot.revision(),
                        words: snapshot.text().unicode_words().count(),
                    };
                    ui.data_mut(|data| data.insert_temp(key, count.clone()));
                    count
                });
            ui.label(
                RichText::new(format!("{} words", count.words))
                    .size(11.0)
                    .color(palette.muted_text),
            );
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(if editor.is_dirty() {
                    "Modified"
                } else {
                    "Unchanged"
                })
                .size(11.0)
                .color(palette.muted_text),
            );
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Key, Modifiers, Pos2, Rect};
    use glyphfolio_editor_core::{ByteOffset, EditCommand, Selection};

    fn frame(
        context: &egui::Context,
        editor: &mut Editor,
        view: &mut NativeViewState,
        config: &EditorConfig,
        events: Vec<Event>,
    ) -> bool {
        let mut result = None;
        let mut output = context.run_ui(
            egui::RawInput {
                focused: true,
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(640.0, 480.0))),
                events,
                ..Default::default()
            },
            |ui| {
                result = Some(
                    EditorPanel::new(editor, view, config)
                        .id(Id::unique("panel-test"))
                        .max_height(400.0)
                        .show_status(false)
                        .show(ui)
                        .map(|response| response.response.changed()),
                );
            },
        );
        output.textures_delta.clear();
        result.unwrap().unwrap()
    }

    #[test]
    fn drop_in_panel_keyboard_formatting_reuses_document_and_history() {
        let context = egui::Context::default();
        let mut editor = Editor::new("word");
        editor
            .set_selection(Selection {
                anchor: ByteOffset(0),
                head: ByteOffset(4),
                ..Selection::default()
            })
            .unwrap();
        let mut view = NativeViewState::default();
        view.request_focus();
        let config = EditorConfig::minimal();
        frame(&context, &mut editor, &mut view, &config, vec![]);
        let selected = editor.selection();
        let changed = frame(
            &context,
            &mut editor,
            &mut view,
            &config,
            vec![Event::Key {
                key: Key::B,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers {
                    command: true,
                    ..Modifiers::NONE
                },
            }],
        );
        assert!(changed);
        assert_eq!(editor.snapshot().text(), "**word**");
        let mut pending = PendingActions::default();
        dispatch_action(
            EditorAction::Undo,
            &mut editor,
            &mut view,
            &context,
            &mut pending,
        )
        .unwrap();
        assert_eq!(editor.snapshot().text(), "word");
        assert_eq!(editor.selection(), selected);
        dispatch_action(
            EditorAction::Redo,
            &mut editor,
            &mut view,
            &context,
            &mut pending,
        )
        .unwrap();
        let revision = editor.snapshot().revision();
        dispatch_action(
            EditorAction::Save,
            &mut editor,
            &mut view,
            &context,
            &mut pending,
        )
        .unwrap();
        dispatch_action(
            EditorAction::ToggleDiagnostics,
            &mut editor,
            &mut view,
            &context,
            &mut pending,
        )
        .unwrap();
        assert!(pending.save_requested);
        assert!(pending.diagnostics_requested);
        assert_eq!(editor.snapshot().revision(), revision);
        assert_eq!(editor.snapshot().text(), "**word**");
        assert!(editor.is_dirty());
    }

    #[test]
    fn invalid_panel_configuration_does_not_mutate_document_or_view() {
        let context = egui::Context::default();
        let mut editor = Editor::new("keep **source**");
        let original = editor.snapshot();
        let mut view = NativeViewState::default();
        let mut config = EditorConfig::default();
        config.document.font_size = f32::NAN;
        let mut invalid = false;
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            invalid = EditorPanel::new(&mut editor, &mut view, &config)
                .show(ui)
                .is_err();
        });
        output.textures_delta.clear();
        assert!(invalid);
        assert!(view.configuration().is_none());
        assert_eq!(editor.snapshot().document_id(), original.document_id());
        assert_eq!(editor.snapshot().revision(), original.revision());
        assert_eq!(editor.snapshot().text(), original.text());
        assert!(!editor.can_undo());
    }

    #[test]
    fn disabled_configured_command_is_refused_by_panel_dispatch() {
        let context = egui::Context::default();
        let mut editor = Editor::new("word");
        let mut view = NativeViewState::default();
        let mut config = EditorConfig::default();
        config.features.bold = false;
        view.configure(&config).unwrap();
        let result = dispatch_action(
            EditorAction::Command(EditCommand::ToggleStrong),
            &mut editor,
            &mut view,
            &context,
            &mut PendingActions::default(),
        );
        assert!(result.is_err());
        assert_eq!(editor.snapshot().text(), "word");
        assert!(!editor.can_undo());
    }
    #[test]
    fn panel_preserves_host_height_for_the_scrollable_writing_surface() {
        let context = egui::Context::default();
        let mut editor = Editor::new(&"A paragraph with some text.\n\n".repeat(100));
        let mut view = NativeViewState::default();
        let config = EditorConfig::glyphfolio();
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                ..Default::default()
            },
            |ui| {
                EditorPanel::new(&mut editor, &mut view, &config)
                    .max_height(600.0)
                    .show_status(false)
                    .show(ui)
                    .unwrap();
            },
        );
        output.textures_delta.clear();
        assert!(
            view.viewport_height > 450.0,
            "writing viewport collapsed to {}",
            view.viewport_height
        );
        assert!(
            view.measured_rows > 12,
            "only {} rows were visible",
            view.measured_rows
        );
        assert_eq!(
            editor.snapshot().revision(),
            glyphfolio_editor_core::Revision(0)
        );
    }
}
