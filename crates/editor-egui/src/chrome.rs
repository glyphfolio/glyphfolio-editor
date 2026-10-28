//! Configurable native editor controls, using GlyphFolio's visual proportions.
//!
//! Controls emit actions; the embedding host owns dispatch, persistence and
//! native input boundaries. Hiding a control never destroys document/history.

use egui::{
    Align, Color32, Context, Layout, Response, RichText, Role, Sense, Stroke, Ui, WidgetInfo, pos2,
    vec2,
};
use glyphfolio_editor_core::{EditCommand, Editor, HeadingLevel, PresentationMode};

use crate::{
    NativeViewState,
    config::{EditorConfig, ThemeMode, ToolbarItem},
    icons::{self, Icon},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorAction {
    Command(EditCommand),
    Undo,
    Redo,
    Presentation(PresentationMode),
    Save,
    EditLink,
    Paste,
    ToggleDiagnostics,
    Settings,
    CancelComposition,
    BeginFreshInput,
}

/// Resolved native colors, including separate accent fill and readable link text.
#[derive(Clone, Copy, Debug)]
pub struct ThemePalette {
    pub is_dark: bool,
    pub app_background: Color32,
    pub editor_background: Color32,
    pub panel_background: Color32,
    pub text: Color32,
    pub secondary_text: Color32,
    pub muted_text: Color32,
    pub disabled_text: Color32,
    pub border: Color32,
    pub strong_border: Color32,
    pub hover: Color32,
    pub selection: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub link: Color32,
    pub code_background: Color32,
    pub code_text: Color32,
    pub success: Color32,
    pub focus_ring: Color32,
}
impl ThemePalette {
    pub fn for_dark(dark: bool) -> Self {
        let rgb = |r, g, b| Color32::from_rgb(r, g, b);
        if dark {
            Self {
                is_dark: true,
                app_background: rgb(26, 27, 30),
                editor_background: rgb(38, 40, 44),
                panel_background: rgb(33, 34, 37),
                text: rgb(240, 241, 243),
                secondary_text: rgb(179, 181, 183),
                muted_text: rgb(160, 163, 170),
                disabled_text: rgb(89, 90, 94),
                border: rgb(55, 57, 61),
                strong_border: rgb(71, 73, 76),
                hover: rgb(51, 53, 57),
                selection: rgb(47, 68, 93),
                accent: rgb(11, 111, 224),
                accent_hover: rgb(42, 130, 234),
                link: rgb(108, 178, 255),
                code_background: rgb(48, 50, 56),
                code_text: rgb(235, 189, 157),
                success: rgb(74, 222, 128),
                focus_ring: rgb(90, 169, 255),
            }
        } else {
            Self {
                is_dark: false,
                app_background: rgb(226, 228, 231),
                editor_background: Color32::WHITE,
                panel_background: rgb(244, 245, 247),
                text: rgb(29, 29, 31),
                secondary_text: rgb(97, 97, 98),
                muted_text: rgb(95, 98, 104),
                disabled_text: rgb(198, 198, 199),
                border: rgb(232, 232, 232),
                strong_border: rgb(219, 219, 219),
                hover: rgb(242, 242, 242),
                selection: rgb(220, 233, 249),
                accent: rgb(10, 97, 204),
                accent_hover: rgb(8, 79, 163),
                link: rgb(10, 97, 204),
                code_background: rgb(226, 228, 231),
                code_text: rgb(186, 74, 0),
                success: rgb(21, 128, 61),
                focus_ring: rgb(10, 95, 209),
            }
        }
    }
    pub fn from_mode(context: &Context, mode: ThemeMode) -> Self {
        Self::for_dark(match mode {
            ThemeMode::Dark => true,
            ThemeMode::Light => false,
            ThemeMode::System => matches!(
                context.system_theme().unwrap_or_else(|| context.theme()),
                egui::Theme::Dark
            ),
        })
    }
    /// Local styling suitable for an embedded editor; other host panels are unaffected.
    pub fn apply_to_ui(&self, ui: &mut Ui) {
        self.apply_visuals(ui.visuals_mut());
        ui.spacing_mut().item_spacing = vec2(4.0, 6.0);
        ui.spacing_mut().button_padding = vec2(6.0, 3.0);
        ui.spacing_mut().interact_size.y = 26.0;
    }
    fn apply_visuals(&self, visuals: &mut egui::Visuals) {
        visuals.dark_mode = self.is_dark;
        visuals.override_text_color = Some(self.text);
        visuals.panel_fill = self.editor_background;
        visuals.window_fill = self.panel_background;
        visuals.extreme_bg_color = self.editor_background;
        visuals.faint_bg_color = self.panel_background;
        visuals.code_bg_color = self.code_background;
        visuals.hyperlink_color = self.link;
        visuals.selection.bg_fill = self.selection;
        visuals.selection.stroke = Stroke::new(1.0, self.text);
        visuals.widgets.noninteractive.fg_stroke.color = self.text;
        visuals.widgets.noninteractive.bg_stroke.color = self.border;
        visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        visuals.widgets.inactive.bg_fill = self.panel_background;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, self.border);
        visuals.widgets.inactive.fg_stroke.color = self.secondary_text;
        visuals.widgets.hovered.bg_fill = self.hover;
        visuals.widgets.hovered.weak_bg_fill = self.hover;
        visuals.widgets.hovered.fg_stroke.color = self.text;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, self.border);
        visuals.widgets.active.bg_fill = self.selection;
        visuals.widgets.active.weak_bg_fill = self.selection;
        visuals.widgets.active.fg_stroke.color = self.text;
    }
}

/// Explicit host opt-in to applying the editor palette to an entire egui context.
/// Embedded users can instead apply `ThemePalette::apply_to_ui` inside a scope.
pub fn apply_style(context: &Context, mode: ThemeMode) -> ThemePalette {
    context.set_theme(match mode {
        ThemeMode::System => egui::ThemePreference::System,
        ThemeMode::Light => egui::ThemePreference::Light,
        ThemeMode::Dark => egui::ThemePreference::Dark,
    });
    let palette = ThemePalette::from_mode(context, mode);
    context.style_mut_of(context.theme(), |style| {
        palette.apply_visuals(&mut style.visuals)
    });
    palette
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EditorToolbar;
impl EditorToolbar {
    /// Render enabled configured groups and return concrete host actions.
    /// Undo/redo and source edits are disabled while composition is pending;
    /// presentation requests are still emitted for the session to defer safely.
    pub fn show(
        ui: &mut Ui,
        editor: &Editor,
        view: &NativeViewState,
        config: &EditorConfig,
    ) -> Vec<EditorAction> {
        if !config.toolbar.visible {
            return Vec::new();
        }
        let palette = ThemePalette::from_mode(ui.ctx(), config.appearance.theme);
        let mut actions = Vec::new();
        ui.scope(|ui| {
            palette.apply_to_ui(ui);
            let edge_left = ui.cursor().min.x;
            let edge_right = edge_left + ui.available_width();
            ui.painter().line_segment(
                [
                    egui::pos2(edge_left, ui.cursor().min.y),
                    egui::pos2(edge_right, ui.cursor().min.y),
                ],
                Stroke::new(1.0, palette.border),
            );
            ui.add_space(6.0);
            let groups: Vec<Vec<_>> = config
                .toolbar
                .groups
                .iter()
                .map(|group| {
                    group
                        .iter()
                        .copied()
                        .filter(|item| {
                            *item != ToolbarItem::Presentation && config.features.allows_item(*item)
                        })
                        .collect::<Vec<_>>()
                })
                .filter(|group| !group.is_empty())
                .collect();
            let presentation = config
                .toolbar
                .groups
                .iter()
                .flatten()
                .any(|item| *item == ToolbarItem::Presentation)
                && config.features.allows_item(ToolbarItem::Presentation);
            let left_width = groups
                .iter()
                .map(|group| {
                    group
                        .iter()
                        .map(|item| {
                            if *item == ToolbarItem::Heading {
                                94.0
                            } else {
                                30.0
                            }
                        })
                        .sum::<f32>()
                })
                .sum::<f32>()
                + groups.len().saturating_sub(1) as f32 * 13.0;
            let right_width = if presentation { 142.0 } else { 30.0 };
            let narrow = left_width + right_width + 12.0 > ui.available_width();
            let paint_groups = |ui: &mut Ui, actions: &mut Vec<EditorAction>| {
                for (index, group) in groups.iter().enumerate() {
                    if index > 0 {
                        separator(ui, palette.border);
                    }
                    for item in group {
                        let width = if *item == ToolbarItem::Heading {
                            94.0
                        } else {
                            26.0
                        };
                        // Tell the wrapping parent the complete control size
                        // before add_enabled_ui creates its local child scope.
                        ui.allocate_ui_with_layout(
                            vec2(width, 26.0),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                item_control(ui, *item, editor, view, config, &palette, actions);
                            },
                        );
                    }
                }
            };
            if narrow {
                ui.horizontal_wrapped(|ui| {
                    paint_groups(ui, &mut actions);
                });
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), 30.0),
                    Layout::right_to_left(Align::Center),
                    |ui| {
                        more_control(ui, view, config, &palette, &mut actions);
                        if presentation {
                            presentation_control(ui, view, config, &palette, &mut actions);
                        }
                    },
                );
            } else {
                ui.horizontal(|ui| {
                    paint_groups(ui, &mut actions);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        more_control(ui, view, config, &palette, &mut actions);
                        if presentation {
                            presentation_control(ui, view, config, &palette, &mut actions);
                        }
                    });
                });
            }
            ui.add_space(6.0);
            ui.painter().line_segment(
                [
                    egui::pos2(edge_left, ui.cursor().min.y),
                    egui::pos2(edge_right, ui.cursor().min.y),
                ],
                Stroke::new(1.0, palette.border),
            );
        });
        actions
    }
}

fn separator(ui: &mut Ui, color: Color32) {
    let (_, rect) = ui.allocate_space(vec2(9.0, 22.0));
    ui.painter().line_segment(
        [
            pos2(rect.center().x, rect.top() + 3.0),
            pos2(rect.center().x, rect.bottom() - 3.0),
        ],
        Stroke::new(1.0, color),
    );
}

fn item_control(
    ui: &mut Ui,
    item: ToolbarItem,
    editor: &Editor,
    view: &NativeViewState,
    _config: &EditorConfig,
    palette: &ThemePalette,
    actions: &mut Vec<EditorAction>,
) {
    let composing = view.is_composing();
    if item == ToolbarItem::Table {
        ui.add_enabled_ui(!composing, |ui| {
            let button = icon_button(ui, Icon::Table, "Table operations", palette);
            egui::Popup::menu(&button).show(|ui| {
                use glyphfolio_editor_core::TableCommand::*;
                for (label, command) in [
                    ("Insert table", Insert),
                    ("Add row", AddRow),
                    ("Delete row", DeleteRow),
                    ("Add column", AddColumn),
                    ("Delete column", DeleteColumn),
                    ("Next cell", NextCell),
                    ("Previous cell", PreviousCell),
                    ("Next row", NextRow),
                    ("Previous row", PreviousRow),
                    ("Clear cell", ClearCell),
                    (
                        "Align column left",
                        Align(glyphfolio_editor_core::ColumnAlignment::Left),
                    ),
                    (
                        "Align column center",
                        Align(glyphfolio_editor_core::ColumnAlignment::Center),
                    ),
                    (
                        "Align column right",
                        Align(glyphfolio_editor_core::ColumnAlignment::Right),
                    ),
                    ("Paragraph after table", Exit),
                    ("Delete table", DeleteTable),
                ] {
                    if ui.button(label).clicked() {
                        actions.push(EditorAction::Command(EditCommand::Table(command)));
                        ui.close();
                    }
                }
            });
        });
        return;
    }
    if item == ToolbarItem::Heading {
        ui.add_enabled_ui(!composing, |ui| {
            let response = egui::ComboBox::from_id_salt("editor-heading")
                .width(90.0)
                .selected_text(
                    RichText::new(view.heading_at_caret(editor).map_or_else(
                        || "Paragraph".to_owned(),
                        |heading| format!("Heading {}", heading.number()),
                    ))
                    .size(11.0),
                )
                .show_ui(ui, |ui| {
                    for (label, heading) in [
                        ("Paragraph", None),
                        ("Heading 1", Some(HeadingLevel::One)),
                        ("Heading 2", Some(HeadingLevel::Two)),
                        ("Heading 3", Some(HeadingLevel::Three)),
                        ("Heading 4", Some(HeadingLevel::Four)),
                        ("Heading 5", Some(HeadingLevel::Five)),
                        ("Heading 6", Some(HeadingLevel::Six)),
                    ] {
                        if ui.button(label).clicked() {
                            actions.push(EditorAction::Command(EditCommand::SetHeading(heading)));
                            ui.close();
                        }
                    }
                });
            response
                .response
                .on_hover_text("Paragraph and heading style");
        });
        return;
    }
    let Some((icon, label, action)) = item_action(item) else {
        return;
    };
    let enabled = !composing
        && match item {
            ToolbarItem::Undo => editor.can_undo(),
            ToolbarItem::Redo => editor.can_redo(),
            _ => true,
        };
    let response = ui
        .add_enabled_ui(enabled, |ui| icon_control(ui, icon, label, palette, matches!(action, EditorAction::Command(command) if view.command_active(editor, command))))
        .inner;
    if response.clicked() {
        actions.push(action);
    }
}

fn item_action(item: ToolbarItem) -> Option<(Icon, &'static str, EditorAction)> {
    let command = |icon, label, command| Some((icon, label, EditorAction::Command(command)));
    match item {
        ToolbarItem::Undo => Some((
            Icon::Undo,
            if cfg!(target_os = "macos") {
                "Undo (⌘Z)"
            } else {
                "Undo (Ctrl+Z)"
            },
            EditorAction::Undo,
        )),
        ToolbarItem::Redo => Some((
            Icon::Redo,
            if cfg!(target_os = "macos") {
                "Redo (⇧⌘Z)"
            } else {
                "Redo (Ctrl+Y)"
            },
            EditorAction::Redo,
        )),
        ToolbarItem::Bold => command(
            Icon::Bold,
            if cfg!(target_os = "macos") {
                "Bold (⌘B)"
            } else {
                "Bold (Ctrl+B)"
            },
            EditCommand::ToggleStrong,
        ),
        ToolbarItem::Italic => command(
            Icon::Italic,
            if cfg!(target_os = "macos") {
                "Italic (⌘I)"
            } else {
                "Italic (Ctrl+I)"
            },
            EditCommand::ToggleEmphasis,
        ),
        ToolbarItem::Strikethrough => command(
            Icon::Strikethrough,
            "Strikethrough",
            EditCommand::ToggleStrikethrough,
        ),
        ToolbarItem::TaskList => command(Icon::TaskList, "Task list", EditCommand::ToggleTaskList),
        ToolbarItem::TaskChecked => command(
            Icon::Check,
            "Toggle task completion",
            EditCommand::ToggleTaskChecked,
        ),
        ToolbarItem::InlineCode => command(
            Icon::InlineCode,
            if cfg!(target_os = "macos") {
                "Inline code (⌘E)"
            } else {
                "Inline code (Ctrl+E)"
            },
            EditCommand::ToggleInlineCode,
        ),
        ToolbarItem::BulletList => command(
            Icon::BulletList,
            "Bullet list",
            EditCommand::ToggleUnorderedList,
        ),
        ToolbarItem::OrderedList => command(
            Icon::OrderedList,
            "Numbered list",
            EditCommand::ToggleOrderedList,
        ),
        ToolbarItem::Blockquote => {
            command(Icon::Blockquote, "Quote", EditCommand::ToggleBlockQuote)
        }
        ToolbarItem::CodeFence => {
            command(Icon::CodeFence, "Code block", EditCommand::ToggleCodeFence)
        }
        ToolbarItem::Indent => command(Icon::Indent, "Indent (Tab)", EditCommand::Indent),
        ToolbarItem::Outdent => command(Icon::Outdent, "Outdent (Shift+Tab)", EditCommand::Outdent),
        ToolbarItem::Paste => Some((
            Icon::File,
            "Paste text, formatting or files",
            EditorAction::Paste,
        )),
        ToolbarItem::Link => Some((Icon::Link, "Insert or edit link", EditorAction::EditLink)),
        ToolbarItem::Save => Some((Icon::Save, "Save document", EditorAction::Save)),
        ToolbarItem::Table | ToolbarItem::Heading | ToolbarItem::Presentation => None,
    }
}

/// Compact native line-icon button with a full accessible name and tooltip.
pub fn icon_button(ui: &mut Ui, icon: Icon, label: &str, palette: &ThemePalette) -> Response {
    icon_control(ui, icon, label, palette, false)
}
fn icon_control(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    palette: &ThemePalette,
    active: bool,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
    if active {
        ui.painter().rect_filled(rect, 4.0, palette.selection);
    }
    response.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), label));
    if response.hovered() && ui.is_enabled() {
        ui.painter().rect_filled(rect, 4.0, palette.hover);
    }
    if response.is_pointer_button_down_on() {
        ui.painter().rect_filled(rect, 4.0, palette.selection);
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            4.0,
            Stroke::new(1.5, palette.focus_ring),
            egui::StrokeKind::Inside,
        );
    }
    icons::centered(
        icon,
        ui.painter(),
        rect,
        if !ui.is_enabled() {
            palette.disabled_text
        } else if response.hovered() {
            palette.text
        } else {
            palette.secondary_text
        },
    );
    response.on_hover_text(label)
}

fn presentation_control(
    ui: &mut Ui,
    view: &NativeViewState,
    config: &EditorConfig,
    palette: &ThemePalette,
    actions: &mut Vec<EditorAction>,
) {
    // Allocate the pill itself, rather than letting a frame's nested horizontal
    // layout claim the remaining row width. The parent can then anchor this
    // exact rectangle from either direction without estimated spacer widths.
    let (rect, _) = ui.allocate_exact_size(vec2(106.0, 30.0), Sense::hover());
    ui.painter().rect_filled(rect, 6.0, palette.border);
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect.shrink(2.0))
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (label, mode, allowed) in [
                (
                    "Rich",
                    PresentationMode::Rich,
                    config.features.rich && config.editor.allow_experimental_rich,
                ),
                ("Source", PresentationMode::Source, config.features.source),
            ] {
                let active = view.presentation() == mode;
                let response = ui.add_enabled(
                    allowed,
                    egui::Button::new(RichText::new(label).size(11.0).color(if active {
                        Color32::WHITE
                    } else {
                        palette.secondary_text
                    }))
                    .fill(if active {
                        palette.accent
                    } else {
                        Color32::TRANSPARENT
                    })
                    .stroke(Stroke::NONE)
                    .corner_radius(4.0)
                    .min_size(vec2(
                        if mode == PresentationMode::Source {
                            58.0
                        } else {
                            44.0
                        },
                        26.0,
                    )),
                );
                response.widget_info(|| {
                    WidgetInfo::selected(Role::RadioButton, allowed, active, label)
                });
                if response.clicked() {
                    actions.push(EditorAction::Presentation(mode));
                }
            }
        },
    );
}

fn more_control(
    ui: &mut Ui,
    view: &NativeViewState,
    config: &EditorConfig,
    palette: &ThemePalette,
    actions: &mut Vec<EditorAction>,
) {
    let response = icon_button(ui, Icon::More, "Editor options", palette);
    egui::Popup::menu(&response).show(|ui| {
        if ui.button("Editor settings").clicked() {
            actions.push(EditorAction::Settings);
            ui.close();
        }
        if ui
            .button(if config.diagnostics.visible {
                "Hide diagnostics"
            } else {
                "Show diagnostics"
            })
            .clicked()
        {
            actions.push(EditorAction::ToggleDiagnostics);
            ui.close();
        }
        if config.diagnostics.native_input_controls {
            ui.separator();
            if ui
                .add_enabled(view.is_composing(), egui::Button::new("Cancel composition"))
                .clicked()
            {
                actions.push(EditorAction::CancelComposition);
                ui.close();
            }
            if ui.button("Begin fresh input").clicked() {
                actions.push(EditorAction::BeginFreshInput);
                ui.close();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(
        context: &Context,
        editor: &Editor,
        view: &NativeViewState,
        config: &EditorConfig,
        click: bool,
    ) -> Vec<EditorAction> {
        let point = egui::pos2(13.0, 13.0);
        let events = if click {
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        } else {
            Vec::new()
        };
        let mut actions = Vec::new();
        let mut output = context.run_ui(
            egui::RawInput {
                events,
                focused: true,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    vec2(280.0, 100.0),
                )),
                ..Default::default()
            },
            |ui| {
                actions = EditorToolbar::show(ui, editor, view, config);
            },
        );
        output.textures_delta.clear();
        actions
    }

    #[test]
    fn actual_toolbar_click_dispatches_shared_formatting_and_undo() {
        let context = Context::default();
        let mut editor = Editor::new("word");
        editor
            .set_selection(glyphfolio_editor_core::Selection {
                anchor: glyphfolio_editor_core::ByteOffset(0),
                head: glyphfolio_editor_core::ByteOffset(4),
                ..Default::default()
            })
            .unwrap();
        let mut view = NativeViewState::default();
        let mut config = EditorConfig::default();
        config.toolbar.groups = vec![vec![ToolbarItem::Bold]];
        assert!(frame(&context, &editor, &view, &config, false).is_empty());
        let actions = frame(&context, &editor, &view, &config, true);
        assert_eq!(actions, [EditorAction::Command(EditCommand::ToggleStrong)]);
        let EditorAction::Command(command) = actions[0] else {
            unreachable!()
        };
        view.execute_command(&mut editor, command, &context)
            .unwrap();
        assert_eq!(editor.snapshot().text(), "**word**");
        editor.undo().unwrap();
        assert_eq!(editor.snapshot().text(), "word");
    }

    #[test]
    fn feature_filter_and_composition_prevent_toolbar_mutation() {
        let context = Context::default();
        let editor = Editor::new("word");
        let mut view = NativeViewState::default();
        let mut config = EditorConfig::default();
        config.toolbar.groups = vec![vec![ToolbarItem::Bold]];
        config.features.bold = false;
        assert!(frame(&context, &editor, &view, &config, true).is_empty());
        config.features.bold = true;
        view.preedit(&editor, "に", None);
        assert!(view.is_composing());
        assert!(frame(&context, &editor, &view, &config, true).is_empty());
        assert_eq!(editor.snapshot().text(), "word");
        assert!(!editor.can_undo());
    }

    #[test]
    fn narrow_toolbars_keep_all_buttons_and_the_following_footer_inside_the_column() {
        for width in [416.0, 256.0] {
            let context = Context::default();
            context.enable_accesskit();
            let editor = Editor::new("word");
            let view = NativeViewState::default();
            let config = EditorConfig::glyphfolio();
            let column = egui::Rect::from_min_size(egui::pos2(32.0, 20.0), vec2(width, 600.0));
            let mut toolbar_rect = egui::Rect::NOTHING;
            let mut footer_rect = egui::Rect::NOTHING;
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        vec2(width + 64.0, 700.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    ui.scope_builder(egui::UiBuilder::new().max_rect(column), |ui| {
                        ui.set_width(width);
                        let start = ui.cursor().min;
                        assert!(EditorToolbar::show(ui, &editor, &view, &config).is_empty());
                        toolbar_rect = egui::Rect::from_min_max(start, ui.min_rect().max);
                        footer_rect = ui
                            .with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label("Preview")
                            })
                            .response
                            .rect;
                    });
                },
            );
            output.textures_delta.clear();
            assert!(
                toolbar_rect.right() <= column.right() + 0.5,
                "width={width} toolbar={toolbar_rect:?} column={column:?}"
            );
            assert!(
                footer_rect.right() <= column.right() + 0.5,
                "width={width} footer={footer_rect:?} column={column:?}"
            );
            let tree = output
                .platform_output
                .accesskit_update
                .as_ref()
                .expect("accessible toolbar");
            let mut labels = Vec::new();
            for (_, node) in &tree.nodes {
                if matches!(node.role(), Role::Button | Role::RadioButton) {
                    let bounds = node.bounds().expect("control geometry");
                    assert!(
                        bounds.x0 >= f64::from(column.left()) - 0.5
                            && bounds.x1 <= f64::from(column.right()) + 0.5,
                        "width={width} label={:?} bounds={bounds:?} column={column:?}",
                        node.label()
                    );
                    labels.push(node.label().unwrap_or_default());
                }
            }
            for expected in [
                "Bold",
                "Italic",
                "Bullet list",
                "Numbered list",
                "Quote",
                "Code block",
                "Indent",
                "Outdent",
                "Save document",
                "Editor options",
                "Rich",
                "Source",
            ] {
                assert!(
                    labels.iter().any(|label| label.starts_with(expected)),
                    "width={width} missing {expected}: {labels:?}"
                );
            }
        }
    }

    #[test]
    fn hidden_toolbar_does_not_change_document_or_history() {
        let context = Context::default();
        let editor = Editor::new("**source**");
        let view = NativeViewState::default();
        let mut config = EditorConfig::default();
        config.toolbar.visible = false;
        let mut output = context.run_ui(Default::default(), |ui| {
            assert!(EditorToolbar::show(ui, &editor, &view, &config).is_empty());
        });
        output.textures_delta.clear();
        assert_eq!(editor.snapshot().text(), "**source**");
        assert!(!editor.can_undo());
    }
}
