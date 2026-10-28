//! Standalone host for the embeddable editor, without application metadata chrome.
use super::Demo;
use eframe::egui::{self, Align, Color32, Layout, RichText, Ui, vec2};
use glyphfolio_editor_core::PresentationMode;
use glyphfolio_editor_egui::{
    EditorPanel,
    chrome::ThemePalette,
    config::{EditorConfig, ThemeMode},
};

impl Demo {
    pub(super) fn show_workspace_ui(&mut self, ui: &mut Ui, context: &egui::Context) {
        if self.config.status.word_count {
            let snapshot = self.editor.snapshot();
            let key = (snapshot.document_id(), snapshot.revision());
            if self.counted_snapshot != Some(key) {
                self.word_count = snapshot.text().split_whitespace().count();
                self.counted_snapshot = Some(key);
            }
        }
        let palette = ThemePalette::from_mode(context, self.config.appearance.theme);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(palette.editor_background).inner_margin(egui::Margin { bottom: 12, ..Default::default() })).show(ui, |ui| {
            palette.apply_to_ui(ui);
            let gutter = self.config.document.padding.min((ui.available_width()-80.0).max(0.0)/2.0);
            let width = self.config.document.max_width.min((ui.available_width()-gutter*2.0).max(80.0));
            ui.with_layout(Layout::top_down(Align::Center), |ui| {
                ui.allocate_ui_with_layout(vec2(width,ui.available_height()), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(width);
                    let footer = if self.config.status.visible { 30.0 } else { 0.0 };
                    let panel_height = (ui.available_height() - footer).max(80.0);
                    match EditorPanel::new(&mut self.editor, &mut self.view, &self.config)
                        .with_gutters(false).show_status(false).max_height(panel_height).show(ui) {
                        Ok(result) => {
                            if result.save_requested { self.request_save(); }
                            if result.settings_requested { self.settings_open = !self.settings_open; }
                            if result.diagnostics_requested { self.diagnostics_visible = !self.diagnostics_visible; }
                        }
                        Err(error) => self.view.last_error = Some(error.to_string()),
                    }
                    if self.config.status.visible {
                        ui.add_space(5.0);
                        ui.horizontal(|ui| {
                            if let Some(message) = &self.save_message { ui.label(RichText::new(message).size(11.0).color(palette.success)); }
                            if self.config.status.word_count { ui.label(RichText::new(format!("{} words",self.word_count)).size(11.0).color(palette.muted_text)); }
                            ui.with_layout(Layout::right_to_left(Align::Center),|ui| {
                                if ui.add(egui::Button::new(RichText::new("Preview").size(11.0).color(palette.muted_text)).frame(false)).on_hover_text("Native editor preview. Screen-reader qualification remains in progress.").clicked() { self.diagnostics_visible = !self.diagnostics_visible; }
                                if self.view.render_metrics().is_some_and(|metrics| metrics.large_rich_source_fallback) { ui.label(RichText::new("Large paragraph shown as Source").size(11.0).color(palette.secondary_text)); }
                                else if self.view.is_composing() && self.view.presentation()==PresentationMode::Rich { ui.label(RichText::new("Source during composition").size(11.0).color(palette.secondary_text)); }
                            });
                        });
                    }
                    if let Some(error) = &self.view.last_error { ui.label(RichText::new(error).size(12.0).color(Color32::from_rgb(190,55,55))); }
                });
            });
        });
        self.show_settings(context);
        self.show_save_dialog(context);
        self.show_diagnostics(context);
    }

    fn request_save(&mut self) {
        if self.view.is_composing() {
            self.view.last_error = Some("Finish or cancel composition before saving".into());
            return;
        }
        if let Some(path) = self.save_path.clone() {
            self.save_to(path);
        } else {
            self.save_dialog_open = true;
        }
    }
    fn save_to(&mut self, path: std::path::PathBuf) {
        match glyphfolio_editor_host_example::save_snapshot_atomic(self.editor.snapshot(), &path) {
            Ok(receipt) => match receipt.acknowledge(&mut self.editor) {
                Ok(_) => {
                    self.save_path = Some(path);
                    self.save_message = Some("Saved".into());
                }
                Err(error) => self.view.last_error = Some(error.to_string()),
            },
            Err(error) => self.view.last_error = Some(error.to_string()),
        }
    }
    fn show_save_dialog(&mut self, context: &egui::Context) {
        if !self.save_dialog_open {
            return;
        }
        let mut open = true;
        let mut save = false;
        egui::Window::new("Save Markdown")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(context, |ui| {
                ui.label("Choose the file for this document.");
                ui.add(egui::TextEdit::singleline(&mut self.save_location).desired_width(360.0));
                ui.add_space(8.0);
                if ui
                    .add_enabled(
                        !self.save_location.trim().is_empty(),
                        egui::Button::new("Save"),
                    )
                    .clicked()
                {
                    save = true;
                }
            });
        self.save_dialog_open = open;
        if save {
            self.save_to(self.save_location.trim().into());
            self.save_dialog_open = false;
        }
    }
    fn show_settings(&mut self, context: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let mut open = true;
        let before = self.config.clone();
        egui::Window::new("Editor settings")
            .open(&mut open)
            .collapsible(false)
            .default_width(340.0)
            .show(context, |ui| {
                ui.label(RichText::new("Appearance").strong());
                ui.horizontal(|ui| {
                    for (label, value) in [
                        ("System", ThemeMode::System),
                        ("Light", ThemeMode::Light),
                        ("Dark", ThemeMode::Dark),
                    ] {
                        ui.selectable_value(&mut self.config.appearance.theme, value, label);
                    }
                });
                ui.add_space(10.0);
                ui.add(
                    egui::Slider::new(&mut self.config.document.font_size, 12.0..=24.0)
                        .text("Text size"),
                );
                ui.add(
                    egui::Slider::new(&mut self.config.document.line_height, 1.2..=2.0)
                        .text("Line spacing"),
                );
                ui.horizontal(|ui| {
                    for (label, width) in
                        [("Narrow", 640.0), ("Comfortable", 760.0), ("Wide", 960.0)]
                    {
                        ui.selectable_value(&mut self.config.document.max_width, width, label);
                    }
                });
                ui.add_space(10.0);
                ui.checkbox(&mut self.config.toolbar.visible, "Formatting toolbar");
                ui.checkbox(&mut self.config.status.visible, "Status bar");
                ui.checkbox(&mut self.config.status.word_count, "Word count");
                ui.checkbox(&mut self.diagnostics_visible, "Diagnostics");
                ui.separator();
                ui.label(RichText::new("Defaults come from your configuration file.").size(12.0));
                ui.label(
                    RichText::new(self.config_path.display().to_string())
                        .size(11.0)
                        .color(ui.visuals().weak_text_color()),
                );
                if ui.button("Reload configuration file").clicked() {
                    match EditorConfig::load(&self.config_path) {
                        Ok(config) => self.config = config,
                        Err(error) => self.view.last_error = Some(error.to_string()),
                    }
                }
            });
        self.settings_open = open;
        if self.config != before {
            match self.view.configure(&self.config) {
                Ok(()) => {
                    glyphfolio_editor_egui::chrome::apply_style(
                        context,
                        self.config.appearance.theme,
                    );
                }
                Err(error) => {
                    self.view.last_error = Some(error.to_string());
                    self.config = before;
                }
            }
        }
    }
    fn show_diagnostics(&mut self, context: &egui::Context) {
        if !self.diagnostics_visible {
            return;
        }
        let mut open = true;
        egui::Window::new("Editor diagnostics")
            .open(&mut open)
            .default_width(360.0)
            .show(context, |ui| {
                ui.label(format!(
                    "Revision {} · {} bytes · {} measured rows",
                    self.editor.snapshot().revision().0,
                    self.editor.snapshot().byte_len(),
                    self.view.measured_rows
                ));
                if let Some(metrics) = self.view.render_metrics() {
                    ui.label(format!(
                        "{} blocks · {} shaped · {} cached",
                        metrics.total_blocks, metrics.shaped_blocks, metrics.cache_entries
                    ));
                    ui.label(format!(
                        "Layout: {:.2} ms · cache: {:.2} MiB",
                        metrics.layout_microseconds as f64 / 1000.0,
                        metrics.cache_bytes as f64 / 1048576.0
                    ));
                }
                if let Some(metrics) = self.view.shaping_diagnostics() {
                    ui.label(format!(
                        "{} fonts · {} missing glyphs",
                        metrics.font_faces, metrics.missing_glyphs
                    ));
                }
                ui.label(format!(
                    "Composing: {} · {} preedit events · {} commits",
                    self.view.is_composing(),
                    self.view.trace.preedit_events,
                    self.view.trace.commit_events
                ));
                if self.config.diagnostics.native_input_controls {
                    ui.horizontal(|ui| {
                        if ui.button("Cancel IME").clicked() {
                            self.view.cancel_composition();
                            context.memory_mut(|memory| memory.interrupt_ime());
                        }
                        if ui.button("Begin fresh input").clicked() {
                            context.memory_mut(|memory| memory.interrupt_ime());
                            let _ = self.view.begin_native_input(&self.editor);
                            self.view.request_focus();
                        }
                    });
                }
            });
        self.diagnostics_visible = open;
    }
}
