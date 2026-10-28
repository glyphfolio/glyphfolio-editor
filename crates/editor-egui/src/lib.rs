//! Original native Markdown editor adapter with shared Source/Rich transactions.
//!
//! COSMIC Text provides low-level shaping and egui provides platform transport.
//! Rich remains explicitly experimental until native qualification is complete.
//! Derived projection/layout strings never become authoritative documents.

pub mod accessibility;
pub mod assets;
pub mod chrome;
pub mod clipboard;
pub mod config;
mod geometry;
pub mod icons;
mod link;
pub mod panel;
use glyphfolio_editor_markdown::editing as rich_edit;
#[cfg(test)]
mod table_tests;
mod typing;
pub use panel::{EditorPanel, EditorPanelResponse};
pub mod input;
mod presentation;
pub use presentation::RenderMetrics;
use presentation::{DecorationKind, PreparedSurface, RenderEngine, SurfaceInput};

use egui::{
    Color32, Event, FontId, Id, Key, Modifiers, Pos2, Rect, Response, Sense, Stroke, Ui, Widget,
    vec2,
};
use geometry::{Geometry, relative, translated};
use glyphfolio_editor_core::{
    Affinity, ByteOffset, EditCommand, Editor, PresentationMode, Selection, Snapshot, Transaction,
    TransactionOrigin,
};
use glyphfolio_editor_layout::{LayoutDiagnostics, LayoutOptions, ShapedDocument, ShapingEngine};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
    time::Duration,
};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeInputTrace {
    pub preedit_events: usize,
    pub commit_events: usize,
    pub text_events: usize,
    pub cancelled_sessions: usize,
    pub rejected_events: usize,
}

/// Actual paint work; scanning exact-fallback geometry is reported separately
/// from visible GPU image submission and indexed style candidates.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativePaintMetrics {
    pub considered_glyphs: usize,
    pub painted_glyphs: usize,
    pub style_candidates: usize,
    pub maximum_style_candidates_per_glyph: usize,
}

struct CachedShape {
    text: String,
    width: u32,
    font_size: u32,
    scale: u32,
    document: Arc<ShapedDocument>,
}

/// One native view with original composition ownership, shaping and semantics.
pub struct NativeViewState {
    input: input::CompositionSession,
    had_focus: bool,
    request_focus: bool,
    preferred_x: Option<f32>,
    last_input_time: f64,
    cached_snapshot: Option<Snapshot>,
    cached_text: String,
    pub last_error: Option<String>,
    pub candidate_rect: Option<Rect>,
    pub measured_rows: usize,
    pub paint_metrics: NativePaintMetrics,
    pub trace: NativeInputTrace,
    shaper: Option<ShapingEngine>,
    shaped: Option<CachedShape>,
    glyph_textures: HashMap<u64, egui::TextureHandle>,
    clipboard: Option<Box<dyn clipboard::ClipboardService>>,
    assets: assets::AssetCache,
    pub accessibility: accessibility::AccessibilityState,
    scroll_to_caret: bool,
    viewport_height: f32,
    external_input_boundary: bool,
    render_engine: RenderEngine,
    last_surface: Option<PreparedSurface>,
    experimental_rich: bool,
    activated_link: Option<String>,
    link_draft: Option<link::LinkDraft>,
    pending_marks: Option<typing::PendingMarks>,
    configuration: Option<config::EditorConfig>,
}
impl Default for NativeViewState {
    fn default() -> Self {
        Self {
            input: input::CompositionSession::default(),
            had_focus: false,
            request_focus: false,
            preferred_x: None,
            last_input_time: 0.0,
            cached_snapshot: None,
            cached_text: String::new(),
            last_error: None,
            candidate_rect: None,
            measured_rows: 0,
            paint_metrics: NativePaintMetrics::default(),
            trace: NativeInputTrace::default(),
            shaper: None,
            shaped: None,
            glyph_textures: HashMap::new(),
            clipboard: None,
            assets: assets::AssetCache::default(),
            accessibility: accessibility::AccessibilityState::default(),
            scroll_to_caret: false,
            viewport_height: 480.0,
            external_input_boundary: false,
            render_engine: RenderEngine::default(),
            last_surface: None,
            experimental_rich: false,
            activated_link: None,
            link_draft: None,
            pending_marks: None,
            configuration: None,
        }
    }
}
impl NativeViewState {
    /// Explicit toolbar/host paste also supports file-only native clipboards.
    pub fn paste_clipboard(
        &mut self,
        editor: &mut Editor,
        context: &egui::Context,
    ) -> Result<(), String> {
        if !self.clipboard_allowed() {
            return Err("Clipboard is disabled".into());
        }
        if self.is_composing() {
            return Err("Finish composition before pasting".into());
        }
        let rich = self.presentation() == PresentationMode::Rich;
        let formatted = self
            .configuration
            .as_ref()
            .is_none_or(|config| config.features.formatted_paste);
        let attachments = self
            .configuration
            .as_ref()
            .is_none_or(|config| config.features.attachments);
        let service = self
            .clipboard
            .as_mut()
            .ok_or("No clipboard service is configured")?;
        let files = if attachments {
            service.read_files().unwrap_or_default()
        } else {
            Vec::new()
        };
        let text = if rich && formatted {
            service
                .read_html()
                .ok()
                .flatten()
                .and_then(|html| glyphfolio_editor_markdown::html_to_markdown(&html).ok())
                .filter(|text| !text.is_empty())
        } else {
            None
        };
        let image = if attachments && files.is_empty() && text.is_none() {
            service.read_image().ok().flatten()
        } else {
            None
        };
        let text = text.or_else(|| service.read_text().ok());
        context.memory_mut(|memory| memory.interrupt_ime());
        self.input.interrupt();
        self.external_input_boundary = true;
        if !files.is_empty() {
            self.assets.import_many(
                editor,
                files.into_iter().map(assets::AssetImport::File).collect(),
            )?;
        } else if let Some(image) = image {
            image.validate()?;
            self.assets
                .import(editor, assets::AssetImport::Image(image))?;
        } else {
            let text = table_input(
                editor,
                &text.ok_or("No supported clipboard content")?,
                self.presentation(),
            );
            if let Some(geometry) = self
                .last_surface
                .as_ref()
                .map(|surface| surface.geometry.clone())
            {
                replace_visible(editor, self, &geometry, &text, TransactionOrigin::Paste)?;
            } else {
                editor
                    .insert_text(&text, TransactionOrigin::Paste)
                    .map_err(|error| error.to_string())?;
            }
        }
        self.refresh_text(editor);
        self.begin_native_input(editor)?;
        self.request_focus();
        Ok(())
    }

    pub fn heading_at_caret(
        &self,
        editor: &Editor,
    ) -> Option<glyphfolio_editor_core::HeadingLevel> {
        self.render_engine.heading_at_caret(editor)
    }
    pub fn command_active(&self, editor: &Editor, command: EditCommand) -> bool {
        self.pending_marks
            .as_ref()
            .is_some_and(|pending| pending.valid(editor) && pending.marks.contains(&command))
            || self.render_engine.command_active(editor, command)
    }

    pub fn begin_link_edit(&mut self, editor: &Editor) -> Result<(), String> {
        if self.is_composing() {
            return Err("Finish composition before editing a link".into());
        }
        if self
            .configuration
            .as_ref()
            .is_some_and(|config| !config.features.links)
        {
            return Err("Links are disabled".into());
        }
        self.link_draft = Some(link::LinkDraft::capture(editor));
        Ok(())
    }
    pub fn show_link_dialog(&mut self, editor: &mut Editor, context: &egui::Context) {
        if self
            .configuration
            .as_ref()
            .is_some_and(|config| !config.features.links)
        {
            self.link_draft = None;
            return;
        }
        let Some(mut draft) = self.link_draft.take() else {
            return;
        };
        let mut open = true;
        let mut action = None;
        egui::Window::new("Edit link")
            .id(Id::unique("glyphfolio-link-dialog"))
            .open(&mut open)
            .collapsible(false)
            .show(context, |ui| {
                ui.label("Display text");
                ui.text_edit_singleline(&mut draft.label);
                ui.checkbox(&mut draft.wiki, "Link to a note (wiki link)");
                ui.label(if draft.wiki {
                    "Note name"
                } else {
                    "URL or relative reference"
                });
                ui.text_edit_singleline(&mut draft.destination);
                ui.horizontal(|ui| {
                    if ui.button("Apply").clicked() {
                        action = Some(false);
                    }
                    if ui.button("Remove link").clicked() {
                        action = Some(true);
                    }
                });
            });
        if let Some(remove) = action {
            if self.is_composing() {
                self.last_error = Some("Finish composition before editing a link".into());
                self.link_draft = Some(draft);
                return;
            }
            context.memory_mut(|memory| memory.interrupt_ime());
            self.input.interrupt();
            self.external_input_boundary = true;
            if let Err(error) = draft.apply(editor, remove) {
                self.last_error = Some(error);
                self.link_draft = Some(draft);
            } else {
                self.refresh_text(editor);
                let _ = self.begin_native_input(editor);
                self.request_focus();
            }
        } else if open {
            self.link_draft = Some(draft);
        }
    }

    /// Install a host-controlled resolver/importer; document/history are retained.
    pub fn set_asset_service(&mut self, service: Box<dyn assets::AssetService>) {
        self.assets = assets::AssetCache::default();
        self.assets.service = Some(service);
    }
    pub fn import_asset(
        &mut self,
        editor: &mut Editor,
        asset: assets::AssetImport,
    ) -> Result<(), String> {
        if self.is_composing() {
            return Err("Finish composition before importing an attachment".into());
        }
        if self
            .configuration
            .as_ref()
            .is_some_and(|config| !config.features.attachments)
        {
            return Err("Attachments are disabled".into());
        }
        self.input.interrupt();
        self.external_input_boundary = true;
        self.assets.import(editor, asset)?;
        self.refresh_text(editor);
        self.begin_native_input(editor)?;
        self.request_focus();
        Ok(())
    }

    /// Apply validated host settings without replacing document, selection,
    /// history, or an active composition. Presentation changes remain deferred.
    pub fn configure(&mut self, config: &config::EditorConfig) -> Result<(), config::ConfigError> {
        config.validate()?;
        let initial = self.configuration.is_none();
        let desired = PresentationMode::from(config.editor.default_presentation);
        let preference_changed = self.configuration.as_ref().is_some_and(|old| {
            old.editor.default_presentation != config.editor.default_presentation
        });
        self.experimental_rich = config.editor.allow_experimental_rich;
        self.configuration = Some(config.clone());
        if let Some(pending) = &mut self.pending_marks {
            pending
                .marks
                .retain(|mark| config.features.allows_command(*mark));
        }
        if self
            .pending_marks
            .as_ref()
            .is_some_and(|pending| pending.marks.is_empty())
        {
            self.pending_marks = None;
        }
        if initial
            || preference_changed
            || !config.features.allows_presentation(self.presentation())
        {
            self.input.request_presentation(desired);
        }
        Ok(())
    }
    pub fn configuration(&self) -> Option<&config::EditorConfig> {
        self.configuration.as_ref()
    }
    fn command_allowed(&self, command: EditCommand) -> bool {
        self.configuration
            .as_ref()
            .is_none_or(|config| config.features.allows_command(command))
    }
    fn check_command(&self, command: EditCommand) -> Result<(), String> {
        if self.command_allowed(command) {
            Ok(())
        } else {
            Err("This editing command is disabled by the editor configuration".into())
        }
    }
    fn clipboard_allowed(&self) -> bool {
        self.configuration
            .as_ref()
            .is_none_or(|config| config.features.clipboard)
    }
    fn history_allowed(&self) -> bool {
        self.configuration
            .as_ref()
            .is_none_or(|config| config.features.history)
    }
    fn apply_edit_command(
        &mut self,
        editor: &mut Editor,
        command: EditCommand,
    ) -> Result<bool, String> {
        if self.presentation() == PresentationMode::Rich
            && editor.selection().is_collapsed()
            && typing::inline(command)
        {
            let snapshot = editor.snapshot();
            let parsed = glyphfolio_editor_markdown::MarkdownParser::with_dialect(
                glyphfolio_editor_markdown::MarkdownDialect::GfmWiki,
            )
            .parse(&snapshot);
            let at = editor.selection().head;
            let active = parsed
                .styles()
                .iter()
                .filter(|span| {
                    span.source_range.start < at
                        && at < span.source_range.end
                        && matches!(
                            (&span.kind, command),
                            (
                                glyphfolio_editor_markdown::StyleKind::Strong,
                                EditCommand::ToggleStrong
                            ) | (
                                glyphfolio_editor_markdown::StyleKind::Emphasis,
                                EditCommand::ToggleEmphasis
                            ) | (
                                glyphfolio_editor_markdown::StyleKind::Strikethrough,
                                EditCommand::ToggleStrikethrough
                            ) | (
                                glyphfolio_editor_markdown::StyleKind::Code,
                                EditCommand::ToggleInlineCode
                            )
                        )
                })
                .min_by_key(|span| span.source_range.end.0 - span.source_range.start.0);
            if let Some(span) = active {
                self.pending_marks = None;
                if command == EditCommand::ToggleInlineCode {
                    let projection =
                        parsed.project(glyphfolio_editor_markdown::ProjectionOptions::default());
                    let start = projection
                        .source_to_display(span.source_range.start, Affinity::After)
                        .map_err(|error| error.to_string())?
                        .0;
                    let end = projection
                        .source_to_display(span.source_range.end, Affinity::Before)
                        .map_err(|error| error.to_string())?
                        .0;
                    let caret = projection
                        .source_to_display(at, Affinity::After)
                        .map_err(|error| error.to_string())?
                        .0
                        .clamp(start, end);
                    let escape = |value: &str| {
                        value
                            .chars()
                            .flat_map(|ch| {
                                if ch.is_ascii_punctuation() {
                                    vec!['\\', ch]
                                } else {
                                    vec![ch]
                                }
                            })
                            .collect::<String>()
                    };
                    let value = escape(&projection.text()[start..end]);
                    let offset = escape(&projection.text()[start..caret]).len();
                    editor
                        .apply(
                            Transaction::new(&snapshot)
                                .replace(span.source_range.clone(), value)
                                .with_selection(Selection::caret(ByteOffset(
                                    span.source_range.start.0 + offset,
                                )))
                                .with_origin(TransactionOrigin::Formatting)
                                .with_grapheme_selection(Affinity::After),
                        )
                        .map_err(|error| error.to_string())?;
                    return Ok(true);
                }
                let source = snapshot.text();
                let raw = &source[span.source_range.start.0..span.source_range.end.0];
                let width = match command {
                    EditCommand::ToggleStrong => 2,
                    EditCommand::ToggleStrikethrough => raw
                        .bytes()
                        .take_while(|byte| *byte == b'~')
                        .count()
                        .clamp(1, 2),
                    EditCommand::ToggleInlineCode => {
                        raw.bytes().take_while(|byte| *byte == b'`').count()
                    }
                    _ => 1,
                };
                if span.source_range.end.0 - span.source_range.start.0 >= width * 2 {
                    editor
                        .apply(
                            Transaction::new(&snapshot)
                                .replace(
                                    span.source_range.start
                                        ..ByteOffset(span.source_range.start.0 + width),
                                    "",
                                )
                                .replace(
                                    ByteOffset(span.source_range.end.0 - width)
                                        ..span.source_range.end,
                                    "",
                                )
                                .with_origin(TransactionOrigin::Formatting)
                                .with_grapheme_selection(Affinity::After),
                        )
                        .map_err(|error| error.to_string())?;
                    return Ok(true);
                }
            }
            let mut marks = self
                .pending_marks
                .take()
                .filter(|pending| pending.valid(editor))
                .map_or_else(Vec::new, |pending| pending.marks);
            if let Some(index) = marks.iter().position(|mark| *mark == command) {
                marks.remove(index);
            } else {
                marks.push(command);
            }
            self.pending_marks =
                (!marks.is_empty()).then(|| typing::PendingMarks::new(editor, marks));
            return Ok(false);
        }
        self.pending_marks = None;
        editor
            .execute(command)
            .map(|change| change.is_some())
            .map_err(|error| error.to_string())
    }
    fn run_edit_command(
        &mut self,
        editor: &mut Editor,
        command: EditCommand,
    ) -> Result<(), String> {
        self.check_command(command)?;
        self.apply_edit_command(editor, command).map(|_| ())
    }
    /// Host history commands use the same native input boundary as formatting.
    pub fn undo(&mut self, editor: &mut Editor, context: &egui::Context) -> Result<bool, String> {
        self.history_command(editor, context, false)
    }
    pub fn redo(&mut self, editor: &mut Editor, context: &egui::Context) -> Result<bool, String> {
        self.history_command(editor, context, true)
    }
    fn history_command(
        &mut self,
        editor: &mut Editor,
        context: &egui::Context,
        redo: bool,
    ) -> Result<bool, String> {
        if !self.history_allowed() {
            return Err("Undo and redo are disabled by the editor configuration".into());
        }
        if self.is_composing() {
            return Err("Finish or cancel composition before changing history".into());
        }
        self.pending_marks = None;
        context.memory_mut(|memory| memory.interrupt_ime());
        self.input.interrupt();
        self.external_input_boundary = true;
        let changed = if redo { editor.redo() } else { editor.undo() }
            .map_err(|error| error.to_string())?
            .is_some();
        self.refresh_text(editor);
        self.begin_native_input(editor)?;
        self.request_focus();
        context.request_repaint();
        Ok(changed)
    }
    pub fn set_clipboard(&mut self, service: Box<dyn clipboard::ClipboardService>) {
        self.clipboard = Some(service);
    }
    /// Execute a host toolbar/menu command at an explicit native input boundary.
    /// Composition is never silently committed or formatted. This interrupts
    /// the backend and quarantines its current batch before restoring focus.
    pub fn execute_command(
        &mut self,
        editor: &mut Editor,
        command: EditCommand,
        context: &egui::Context,
    ) -> Result<bool, String> {
        self.check_command(command)?;
        if self.is_composing() {
            return Err("Finish or cancel composition before formatting".into());
        }
        context.memory_mut(|memory| memory.interrupt_ime());
        self.input.interrupt();
        self.external_input_boundary = true;
        let changed = self.apply_edit_command(editor, command)?;
        self.refresh_text(editor);
        self.begin_native_input(editor)?;
        self.request_focus();
        context.request_repaint();
        Ok(changed)
    }
    pub fn shaping_diagnostics(&self) -> Option<&LayoutDiagnostics> {
        self.last_surface
            .as_ref()
            .map(|surface| &surface.geometry.document.diagnostics)
            .or_else(|| {
                self.shaped
                    .as_ref()
                    .map(|shape| &shape.document.diagnostics)
            })
    }
    /// Explicit opt-in while native screen-reader qualification is incomplete.
    pub fn enable_experimental_rich(&mut self, enabled: bool) {
        self.experimental_rich = enabled;
    }
    pub fn experimental_rich_enabled(&self) -> bool {
        self.experimental_rich
    }
    pub fn rendered_presentation(&self) -> PresentationMode {
        self.last_surface
            .as_ref()
            .map_or(PresentationMode::Source, |surface| surface.rendered_mode)
    }
    pub fn render_metrics(&self) -> Option<&RenderMetrics> {
        self.last_surface.as_ref().map(|surface| &surface.metrics)
    }
    /// A deliberate modifier-click reports a destination; the host chooses its
    /// URI policy and external handler. The library never executes/open links.
    pub fn take_activated_link(&mut self) -> Option<String> {
        self.activated_link.take()
    }
    pub fn load_font_file(&mut self, path: &Path) -> Result<(), String> {
        self.shaper
            .get_or_insert_with(ShapingEngine::new)
            .load_font_file(path)?;
        self.shaped = None;
        self.glyph_textures.clear();
        Ok(())
    }
    fn shape(&mut self, text: &str, width: f32, font_size: f32, scale: f32) -> Arc<ShapedDocument> {
        if let Some(cached) = &self.shaped
            && cached.text == text
            && cached.width == width.to_bits()
            && cached.font_size == font_size.to_bits()
            && cached.scale == scale.to_bits()
        {
            return cached.document.clone();
        }
        let document = Arc::new(self.shaper.get_or_insert_with(ShapingEngine::new).shape(
            text,
            LayoutOptions {
                width,
                font_size,
                line_height: font_size * 1.45,
                scale,
            },
        ));
        self.shaped = Some(CachedShape {
            text: text.into(),
            width: width.to_bits(),
            font_size: font_size.to_bits(),
            scale: scale.to_bits(),
            document: document.clone(),
        });
        document
    }
    pub fn request_focus(&mut self) {
        self.request_focus = true;
    }
    pub fn is_composing(&self) -> bool {
        self.input.is_composing()
    }
    pub fn presentation(&self) -> PresentationMode {
        self.input.presentation()
    }
    pub fn request_presentation(&mut self, mode: PresentationMode) {
        if mode != self.presentation() {
            self.pending_marks = None;
        }
        if self
            .configuration
            .as_ref()
            .is_some_and(|config| !config.features.allows_presentation(mode))
        {
            self.last_error =
                Some("This presentation is disabled by the editor configuration".into());
            return;
        }
        self.input.request_presentation(mode);
    }
    pub fn can_close(&self) -> bool {
        self.input.can_close()
    }
    /// Explicit host/user boundary, after interrupting/draining the previous backend batch.
    /// Never call this merely because an unowned Commit arrived.
    pub fn begin_native_input(&mut self, editor: &Editor) -> Result<(), String> {
        self.input
            .begin_input(editor)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
    pub fn cancel_composition(&mut self) {
        self.input.interrupt();
        self.trace.cancelled_sessions += 1;
    }
    fn refresh_text(&mut self, editor: &Editor) {
        if let Err(error) = self.input.synchronize(editor) {
            self.last_error = Some(error.to_string());
        }
        let snapshot = editor.snapshot();
        if self.cached_snapshot.as_ref().is_none_or(|old| {
            old.document_id() != snapshot.document_id() || old.revision() != snapshot.revision()
        }) {
            self.cached_text = snapshot.text();
            self.cached_snapshot = Some(snapshot);
        }
    }
    fn display(&self, selection: Selection) -> input::CompositionDisplay {
        if let Some(snapshot) = &self.cached_snapshot
            && let Ok(display) = self.input.display(snapshot, selection)
        {
            return display;
        }
        input::CompositionDisplay {
            text: self.cached_text.clone(),
            caret_byte: selection.head.0,
            preedit_range: None,
        }
    }
    #[allow(clippy::too_many_arguments)] // One immutable UI geometry/input snapshot.
    fn prepare_surface(
        &mut self,
        editor: &mut Editor,
        display: &input::CompositionDisplay,
        width: f32,
        font_size: f32,
        scale: f32,
        viewport_top: f32,
        viewport_bottom: f32,
        accessibility: bool,
    ) -> PreparedSurface {
        let mode = if self.experimental_rich {
            self.presentation()
        } else {
            PresentationMode::Source
        };
        let input = SurfaceInput {
            source_display: &display.text,
            source_caret: display.caret_byte,
            preedit_range: display.preedit_range.clone(),
            selection: editor.selection(),
            mode,
            width,
            font_size,
            line_height_multiplier: self
                .configuration
                .as_ref()
                .map_or(1.45, |config| config.document.line_height),
            glyphfolio_typography: self.configuration.is_some(),
            scale,
            viewport_top,
            viewport_bottom,
            accessibility,
        };
        let images = if self
            .configuration
            .as_ref()
            .is_none_or(|config| config.features.images)
        {
            self.assets.refresh(editor)
        } else {
            HashMap::new()
        };
        self.render_engine.set_images(images);
        let result = self.render_engine.prepare(
            editor,
            input,
            self.shaper.get_or_insert_with(ShapingEngine::new),
        );
        let surface = match result {
            Ok(surface) => surface,
            Err(error) => {
                self.last_error = Some(format!("Presentation uses Source fallback: {error}"));
                let shaped = self.shape(&display.text, width, font_size, scale);
                PreparedSurface {
                    geometry: Geometry::from_shaped(Id::unique("fallback"), shaped),
                    paint_caret_display: display.caret_byte,
                    preedit_display_range: display.preedit_range.clone(),
                    images: Vec::new(),
                    decorations: Vec::new(),
                    paint_spans: Vec::new(),
                    metrics: RenderMetrics::default(),
                    corrected_scroll_top: viewport_top,
                    rendered_mode: PresentationMode::Source,
                }
            }
        };
        self.last_surface = Some(surface.clone());
        surface
    }
    #[cfg(test)]
    fn preedit(&mut self, editor: &Editor, text: &str, active: Option<std::ops::Range<usize>>) {
        if self.input.epoch().is_none() && !text.is_empty() {
            self.begin_native_input(editor).unwrap();
        }
        let epoch = self.input.epoch().unwrap();
        self.input.preedit(editor, epoch, text, active).unwrap();
    }
    #[cfg(test)]
    fn commit(&mut self, editor: &mut Editor, text: &str) -> Result<(), String> {
        let epoch = self.input.epoch().ok_or("No native input owner")?;
        self.input
            .commit(editor, epoch, text)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// Custom original controller; egui supplies events, low-level font layout and paint.
pub struct EditorWidget<'a> {
    editor: &'a mut Editor,
    state: &'a mut NativeViewState,
    id: Id,
    font: FontId,
}
impl<'a> EditorWidget<'a> {
    pub fn new(editor: &'a mut Editor, state: &'a mut NativeViewState) -> Self {
        let font_size = state
            .configuration
            .as_ref()
            .map_or(19.0, |config| config.document.font_size);
        Self {
            editor,
            state,
            id: Id::unique("glyphfolio-native-editor"),
            font: FontId::proportional(font_size),
        }
    }
    pub fn id(mut self, id: Id) -> Self {
        self.id = id;
        self
    }
    /// Set the Source prototype's size. The native system sans-serif/fallback
    /// stack is selected by the shaper; `font.family` is not yet configurable.
    pub fn font(mut self, font: FontId) -> Self {
        self.font = font;
        self
    }
}

impl Widget for EditorWidget<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        self.state.refresh_text(self.editor);
        let width = ui.available_width().max(80.0);
        self.state.viewport_height = ui.clip_rect().height().max(self.font.size * 2.0);
        let palette = self
            .state
            .configuration
            .as_ref()
            .map(|config| chrome::ThemePalette::from_mode(ui.ctx(), config.appearance.theme));
        let color = palette
            .as_ref()
            .map_or(ui.visuals().text_color(), |palette| palette.text);
        let padding = self
            .state
            .configuration
            .as_ref()
            .map_or(16.0, |config| config.document.inner_padding);
        let draw_frame = self
            .state
            .configuration
            .as_ref()
            .is_none_or(|config| config.appearance.frame);
        let prepared_selection = self.editor.selection();
        let accessibility_active = ui.ctx().has_accesskit_node(egui::accesskit_root_id());
        let origin_hint = ui.cursor().min + vec2(padding, padding);
        let viewport_top = (ui.clip_rect().top() - origin_hint.y).max(0.0);
        let viewport_bottom = (ui.clip_rect().bottom() - origin_hint.y).max(viewport_top + 1.0);
        let display = self.state.display(self.editor.selection());
        let surface = self.state.prepare_surface(
            self.editor,
            &display,
            (width - padding * 2.0).max(48.0),
            self.font.size,
            ui.ctx().pixels_per_point(),
            viewport_top,
            viewport_bottom,
            accessibility_active,
        );
        let geometry = surface.geometry.clone();
        let galley = geometry.document.clone();
        let minimum_height = if self.state.configuration.is_some() {
            self.state.viewport_height.max(240.0)
        } else {
            240.0
        };
        let (_, rect) = ui.allocate_space(vec2(
            width.max(galley.width + padding * 2.0),
            galley.height.max(minimum_height) + padding * 2.0,
        ));
        let mut response = ui.interact(rect, self.id, Sense::click_and_drag());
        let origin = rect.min + vec2(padding, padding);
        if self.state.request_focus {
            response.request_focus();
            self.state.request_focus = false;
        }
        ui.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                self.id,
                egui::EventFilter {
                    tab: self
                        .state
                        .configuration
                        .as_ref()
                        .is_none_or(|config| config.features.indentation),
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            );
        });
        let pressed_here = ui.input(|input| {
            input.pointer.primary_pressed()
                && input
                    .pointer
                    .interact_pos()
                    .is_some_and(|point| rect.contains(point) && ui.clip_rect().contains(point))
        });
        if pressed_here {
            self.state.pending_marks = None;
            response.request_focus();
            if self
                .state
                .configuration
                .as_ref()
                .is_none_or(|config| config.features.links)
                && ui.input(|input| input.modifiers.command)
                && let Some(point) = ui.input(|input| input.pointer.interact_pos())
            {
                self.state.activated_link = geometry.link_at(relative(point, origin));
            }
        }
        let task_click = if pressed_here
            && !self.state.is_composing()
            && self
                .state
                .configuration
                .as_ref()
                .is_none_or(|config| config.features.task_lists)
        {
            ui.input(|input| input.pointer.interact_pos())
                .and_then(|point| geometry.task_at(relative(point, origin)))
        } else {
            None
        };
        if let Some(at) = task_click
            && let Err(error) = self
                .editor
                .set_selection(Selection::caret(at))
                .map_err(|error| error.to_string())
                .and_then(|_| {
                    self.state
                        .run_edit_command(self.editor, EditCommand::ToggleTaskChecked)
                })
        {
            self.state.last_error = Some(error);
        }
        if task_click.is_none()
            && !self.state.is_composing()
            && (pressed_here || response.dragged())
            && let Some(point) = ui.input(|input| input.pointer.interact_pos())
        {
            let hit = geometry.hit_test_caret(relative(point, origin));
            let byte = hit.byte.0;
            let mut selected = self.editor.selection();
            if pressed_here && !ui.input(|input| input.modifiers.shift) {
                selected.anchor = ByteOffset(byte);
            }
            selected.head = ByteOffset(byte);
            selected.affinity = hit.affinity;
            if let Err(error) = self.editor.set_selection(selected) {
                self.state.last_error = Some(error.to_string());
            }
            self.state.preferred_x = None;
        }
        if !self.state.is_composing()
            && (response.double_clicked() || response.triple_clicked())
            && let Some(point) = response.interact_pointer_pos()
        {
            let hit = geometry.hit_test_caret(relative(point, origin));
            let text = self.editor.snapshot().text();
            let selected = if response.triple_clicked() {
                logical_line_range(&text, hit.byte.0)
            } else {
                word_selection_range(&text, hit.byte.0)
            };
            let _ = self.editor.set_selection(Selection {
                anchor: ByteOffset(selected.start),
                head: ByteOffset(selected.end),
                affinity: hit.affinity,
            });
        }
        if response.dragged()
            && let Some(point) = ui.input(|input| input.pointer.interact_pos())
            && (point.y < ui.clip_rect().top() || point.y > ui.clip_rect().bottom())
        {
            ui.scroll_to_rect(Rect::from_min_size(point, vec2(1.0, self.font.size)), None);
            ui.ctx().request_repaint();
        }
        let focused = response.has_focus();
        let owns_ime = ui.memory(|memory| memory.owns_ime_events(self.id));
        let acquired_focus = focused && !self.state.had_focus;
        if !focused && self.state.had_focus {
            self.state.cancel_composition();
            ui.memory_mut(|memory| memory.interrupt_ime());
        }
        let quarantine_ime = acquired_focus
            || pressed_here
            || std::mem::take(&mut self.state.external_input_boundary);
        if focused
            && (acquired_focus || pressed_here)
            && let Err(error) = self.state.begin_native_input(self.editor)
        {
            self.state.last_error = Some(error);
        }
        self.state.had_focus = focused;
        let batch_epoch = self.state.input.epoch();
        if accessibility_active {
            self.state.accessibility.prepare_shared(
                self.id,
                rect,
                origin,
                self.editor.snapshot().document_id(),
                &galley,
                accessible_selection(
                    &geometry,
                    self.editor.selection(),
                    self.state
                        .is_composing()
                        .then_some(surface.paint_caret_display),
                ),
            );
        }
        let mut intents = Vec::new();
        ui.input_mut(|input| {
            input.events.retain(|event| {
                let ours = match event {
                    Event::AccessKitActionRequest(request) => {
                        accessibility_active
                            && request.target_tree == accesskit::TreeId::ROOT
                            && self.state.accessibility.contains_node(request.target_node)
                    }
                    Event::Text(_)
                    | Event::Paste(_)
                    | Event::PasteImage(_)
                    | Event::Copy
                    | Event::Cut => focused,
                    Event::Ime(_) => owns_ime,
                    Event::Key {
                        key, pressed: true, ..
                    } => focused && handled_key(*key),
                    Event::WindowFocused(false) => self.state.is_composing(),
                    _ => false,
                };
                if ours {
                    intents.push(event.clone());
                } else if focused && matches!(event, Event::Key { .. }) {
                    // Preserve an intervening key boundary for duplicate-IME
                    // suppression, while leaving unhandled keys to the host.
                    intents.push(event.clone());
                }
                // Window focus is a broadcast event, not owned by this widget.
                !ours || matches!(event, Event::WindowFocused(_))
            })
        });
        let before = self.editor.snapshot().revision();
        let mut copy = None;
        let mut event_geometry = geometry.clone();
        let mut geometry_revision = before;
        let mut geometry_selection = prepared_selection;
        for event in intents {
            if !matches!(event, Event::Text(_)) {
                self.state.input.clear_duplicate();
            }
            self.state.last_input_time = ui.input(|input| input.time);
            let selection_before_event = self.editor.selection();
            let revision_before_event = self.editor.snapshot().revision();
            let trusted_command = matches!(
                &event,
                Event::Text(_)
                    | Event::Paste(_)
                    | Event::PasteImage(_)
                    | Event::Cut
                    | Event::Key { .. }
                    | Event::AccessKitActionRequest(_)
            );
            if self.editor.snapshot().revision() != geometry_revision
                || self.editor.selection() != geometry_selection
            {
                self.state.refresh_text(self.editor);
                let display = self.state.display(self.editor.selection());
                let surface = self.state.prepare_surface(
                    self.editor,
                    &display,
                    (width - padding * 2.0).max(48.0),
                    self.font.size,
                    ui.ctx().pixels_per_point(),
                    viewport_top,
                    viewport_bottom,
                    accessibility_active,
                );
                event_geometry = surface.geometry;
                geometry_revision = self.editor.snapshot().revision();
                geometry_selection = self.editor.selection();
                if accessibility_active {
                    self.state.accessibility.prepare_shared(
                        self.id,
                        rect,
                        origin,
                        self.editor.snapshot().document_id(),
                        &event_geometry.document,
                        accessible_selection(&event_geometry, geometry_selection, None),
                    );
                }
            }
            let result = match event {
                Event::Text(text) => {
                    // Suppress at most one immediate backend duplicate, not all
                    // identical ordinary typing later in the same frame.
                    self.state.trace.text_events += 1;
                    let duplicate = self.state.input.should_suppress_duplicate_text(&text);
                    if duplicate || self.state.is_composing() {
                        Ok(())
                    } else {
                        let text = table_input(self.editor, &text, self.state.presentation());
                        replace_visible(
                            self.editor,
                            self.state,
                            &event_geometry,
                            &text,
                            TransactionOrigin::Typing,
                        )
                    }
                }
                Event::Paste(_) | Event::PasteImage(_) | Event::Copy | Event::Cut
                    if !self.state.clipboard_allowed() =>
                {
                    Err("Clipboard commands are disabled by the editor configuration".into())
                }
                Event::PasteImage(image) if !self.state.is_composing() => {
                    if image.size[0]
                        .checked_mul(image.size[1])
                        .is_none_or(|pixels| {
                            pixels == 0
                                || pixels > assets::MAX_IMAGE_PIXELS
                                || pixels != image.pixels.len()
                        })
                    {
                        Err("Clipboard image exceeds the pixel limit".into())
                    } else {
                        let rgba = image
                            .pixels
                            .iter()
                            .flat_map(|pixel| pixel.to_srgba_unmultiplied())
                            .collect();
                        self.state.import_asset(
                            self.editor,
                            assets::AssetImport::Image(assets::AssetImage {
                                size: image.size,
                                rgba,
                            }),
                        )
                    }
                }
                Event::Paste(text) if !self.state.is_composing() => {
                    // egui-winit normalizes CRLF before delivering Paste. Recover
                    // the exact canonical source only when our host's raw text
                    // is equivalent to that captured event. A changed clipboard
                    // or programmatic event keeps its supplied payload.
                    let raw = self.state.clipboard.as_mut().and_then(|service| {
                        service
                            .read_text()
                            .ok()
                            .filter(|raw| raw.replace("\r\n", "\n") == text)
                    });
                    let formatted = if raw.is_some()
                        && self.state.presentation() == PresentationMode::Rich
                        && self
                            .state
                            .configuration
                            .as_ref()
                            .is_none_or(|config| config.features.formatted_paste)
                    {
                        self.state
                            .clipboard
                            .as_mut()
                            .and_then(|service| service.read_html().ok().flatten())
                            .and_then(|html| {
                                glyphfolio_editor_markdown::html_to_markdown(&html).ok()
                            })
                            .filter(|markdown| !markdown.is_empty())
                    } else {
                        None
                    };
                    let text = table_input(
                        self.editor,
                        formatted.as_deref().or(raw.as_deref()).unwrap_or(&text),
                        self.state.presentation(),
                    );
                    replace_visible(
                        self.editor,
                        self.state,
                        &event_geometry,
                        &text,
                        TransactionOrigin::Paste,
                    )
                }
                Event::Copy | Event::Cut => {
                    let cut = matches!(event, Event::Cut);
                    if cut && self.state.is_composing() {
                        Err("Finish or cancel composition before cutting committed source".into())
                    } else if let Some(service) = self.state.clipboard.as_mut() {
                        let published = clipboard::ClipboardRequest::capture(self.editor)
                            .publish(self.editor, service.as_mut(), false)
                            .map_err(|error| error.to_string());
                        match published {
                            Ok(clipboard::ClipboardResult::EmptySelection) => Ok(()),
                            Ok(_) if cut => replace_visible(
                                self.editor,
                                self.state,
                                &event_geometry,
                                "",
                                TransactionOrigin::Delete,
                            ),
                            Ok(_) => Ok(()),
                            Err(error) => Err(error),
                        }
                    } else if cut {
                        Err("Cut requires a host clipboard service with successful-write acknowledgement".into())
                    } else {
                        let selection = self.editor.selection();
                        if !selection.is_collapsed() {
                            copy = Some(
                                self.editor
                                    .snapshot()
                                    .slice(selection.range())
                                    .map_err(|error| error.to_string())
                                    .unwrap_or_default(),
                            );
                        }
                        Ok(())
                    }
                }
                Event::Ime(event) => {
                    if quarantine_ime {
                        self.state.trace.rejected_events += 1;
                        Err("Discarded an IME event from the focus-transition batch".into())
                    } else if let Some(epoch) = batch_epoch {
                        match event {
                            egui::ImeEvent::Preedit {
                                text,
                                active_range_chars,
                            } => {
                                self.state.trace.preedit_events += 1;
                                self.state
                                    .input
                                    .preedit(self.editor, epoch, &text, active_range_chars)
                                    .map_err(|error| error.to_string())
                            }
                            egui::ImeEvent::Commit(text) => {
                                self.state.trace.commit_events += 1;
                                if !text.is_empty()
                                    && let Some(pending) = self
                                        .state
                                        .pending_marks
                                        .take()
                                        .filter(|pending| pending.valid(self.editor))
                                {
                                    let (replacement, caret, keep) = pending.materialize(&text);
                                    let result = self
                                        .state
                                        .input
                                        .commit_prepared(
                                            self.editor,
                                            epoch,
                                            &text,
                                            &replacement,
                                            caret,
                                        )
                                        .map(|_| ())
                                        .map_err(|error| error.to_string());
                                    if result.is_ok() && keep {
                                        self.state.pending_marks = Some(typing::PendingMarks::new(
                                            self.editor,
                                            pending.marks,
                                        ));
                                    }
                                    result
                                } else {
                                    self.state
                                        .input
                                        .commit(self.editor, epoch, &text)
                                        .map(|_| ())
                                        .map_err(|error| error.to_string())
                                }
                            }
                            egui::ImeEvent::DeleteSurrounding {
                                before_chars,
                                after_chars,
                            } => self
                                .state
                                .input
                                .delete_surrounding(self.editor, epoch, before_chars, after_chars)
                                .map(|_| ())
                                .map_err(|error| error.to_string()),
                            _ => Ok(()), // Deprecated Enabled/Disabled are not emitted by this backend.
                        }
                    } else {
                        self.state.trace.rejected_events += 1;
                        Err("IME input has no active owner; begin a fresh input session".into())
                    }
                }
                Event::Key {
                    key,
                    modifiers,
                    pressed: true,
                    ..
                } if !self.state.is_composing() => {
                    key_command(self.editor, self.state, key, modifiers, &event_geometry)
                }
                Event::Key {
                    key: Key::Escape, ..
                }
                | Event::WindowFocused(false) => {
                    self.state.cancel_composition();
                    ui.memory_mut(|memory| memory.interrupt_ime());
                    Ok(())
                }
                Event::AccessKitActionRequest(request)
                    if request.action == accesskit::Action::ScrollIntoView =>
                {
                    if let Some(bounds) = self
                        .state
                        .accessibility
                        .bounds_for_node(request.target_node)
                    {
                        ui.scroll_to_rect(bounds, Some(egui::Align::Center));
                        Ok(())
                    } else {
                        Err("Unknown accessibility scroll target".into())
                    }
                }
                Event::AccessKitActionRequest(request) if !self.state.is_composing() => {
                    accessibility_action(
                        self.editor,
                        self.state,
                        &request,
                        self.id,
                        &event_geometry,
                        &event_geometry.document.text,
                    )
                }
                _ => Ok(()),
            };
            if let Err(error) = result {
                self.state.last_error = Some(error);
            }
            let command_changed = self.editor.snapshot().revision() != revision_before_event
                || self.editor.selection() != selection_before_event;
            if trusted_command && command_changed && !self.state.is_composing() && focused {
                let _ = self.state.begin_native_input(self.editor);
            }
            if command_changed {
                self.state.scroll_to_caret = true;
            }
        }
        if let Some(text) = copy {
            ui.ctx().copy_text(text);
        }
        if self.editor.snapshot().revision() != before {
            response.mark_changed();
        }
        if response.hovered() && !self.state.is_composing() {
            let files = ui.input(|input| input.raw.dropped_files.clone());
            for file in files {
                let asset = assets::AssetImport::File(file.path().to_path_buf());
                if let Err(error) = self.state.import_asset(self.editor, asset) {
                    self.state.last_error = Some(error);
                }
            }
        }
        self.state.refresh_text(self.editor);
        let paint_display = self.state.display(self.editor.selection());
        let surface = self.state.prepare_surface(
            self.editor,
            &paint_display,
            (width - padding * 2.0).max(48.0),
            self.font.size,
            ui.ctx().pixels_per_point(),
            viewport_top,
            viewport_bottom,
            accessibility_active,
        );
        let geometry = &surface.geometry;
        let galley = &geometry.document;
        self.state.measured_rows = geometry.document.rows.len();
        let painter = ui.painter_at(rect);
        painter.rect_filled(
            rect,
            if draw_frame { 6.0 } else { 0.0 },
            palette
                .as_ref()
                .map_or(ui.visuals().extreme_bg_color, |palette| {
                    palette.editor_background
                }),
        );
        if draw_frame {
            painter.rect_stroke(
                rect,
                6.0,
                Stroke::new(
                    1.0,
                    if focused {
                        ui.visuals().selection.stroke.color
                    } else {
                        ui.visuals().widgets.noninteractive.bg_stroke.color
                    },
                ),
                egui::StrokeKind::Inside,
            );
        }
        for decoration in &surface.decorations {
            let bounds = translated(decoration.bounds, origin);
            match &decoration.kind {
                DecorationKind::CodeBackground | DecorationKind::InlineCode => {
                    painter.rect_filled(
                        bounds,
                        6.0,
                        palette
                            .as_ref()
                            .map_or(ui.visuals().code_bg_color, |palette| {
                                palette.code_background
                            }),
                    );
                }
                DecorationKind::QuoteBar => {
                    painter.rect_filled(
                        bounds,
                        0.0,
                        palette
                            .as_ref()
                            .map_or(ui.visuals().weak_text_color(), |palette| palette.accent),
                    );
                }
                DecorationKind::Bullet(label) => {
                    painter.text(
                        bounds.left_top(),
                        egui::Align2::LEFT_TOP,
                        label,
                        self.font.clone(),
                        color,
                    );
                }
                DecorationKind::TableCell { header, range } => {
                    let at = geometry.to_display(
                        self.editor.selection().head.0,
                        self.editor.selection().affinity,
                    );
                    let active = focused && range.start <= at && at <= range.end;
                    if *header {
                        painter.rect_filled(
                            bounds,
                            0.0,
                            palette
                                .as_ref()
                                .map_or(ui.visuals().faint_bg_color, |palette| {
                                    palette.code_background
                                }),
                        );
                    }
                    painter.rect_stroke(
                        bounds,
                        0.0,
                        Stroke::new(
                            if active { 1.5 } else { 1.0 },
                            palette.as_ref().map_or(
                                ui.visuals().widgets.noninteractive.bg_stroke.color,
                                |palette| {
                                    if active {
                                        palette.accent
                                    } else {
                                        palette.border
                                    }
                                },
                            ),
                        ),
                        egui::StrokeKind::Inside,
                    );
                }
                DecorationKind::Rule => {
                    painter.line_segment(
                        [bounds.left_center(), bounds.right_center()],
                        Stroke::new(1.0, ui.visuals().weak_text_color()),
                    );
                }
            }
        }
        for image in &surface.images {
            if let Some(texture) = self.state.assets.texture(ui.ctx(), &image.destination) {
                painter.image(
                    texture,
                    translated(image.bounds, origin),
                    Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
        }
        if !self.state.is_composing() {
            let selected = self.editor.selection().range();
            for selected_rect in geometry.selection_rects(selected.start.0..selected.end.0) {
                painter.rect_filled(
                    translated(selected_rect, origin),
                    0.0,
                    palette
                        .as_ref()
                        .map_or(ui.visuals().selection.bg_fill, |palette| palette.selection),
                );
            }
        }
        paint_shaped(
            ui.ctx(),
            &painter,
            self.state,
            galley,
            origin,
            color,
            &surface.paint_spans,
        );
        let caret = translated(
            geometry.display_caret_rect(
                surface.paint_caret_display,
                self.editor.selection().affinity,
            ),
            origin,
        );
        if let Some(range) = surface.preedit_display_range.clone() {
            for underline in geometry.display_selection_rects(range) {
                let underline = translated(underline, origin);
                painter.line_segment(
                    [underline.left_bottom(), underline.right_bottom()],
                    Stroke::new(1.5, Color32::LIGHT_BLUE),
                );
            }
        }
        if owns_ime && ui.input(|input| input.focused) {
            let time = ui.input(|input| input.time);
            if self.state.is_composing()
                || ((time - self.state.last_input_time) * 2.0).floor() as i64 % 2 == 0
            {
                painter.line_segment(
                    [caret.left_top(), caret.left_bottom()],
                    Stroke::new(
                        if self.state.configuration.is_some() {
                            2.0
                        } else {
                            1.8
                        },
                        color,
                    ),
                );
            }
            ui.ctx().request_repaint_after(Duration::from_millis(250));
            self.state.candidate_rect = Some(caret);
            ui.ctx().output_mut(|output| {
                output.ime = Some(egui::output::IMEOutput {
                    purpose: egui::IMEPurpose::Normal,
                    rect,
                    cursor_rect: caret,
                    should_interrupt_composition: false,
                })
            });
        } else {
            self.state.candidate_rect = None;
        }
        let accessible_selection = accessible_selection(
            geometry,
            self.editor.selection(),
            self.state
                .is_composing()
                .then_some(surface.paint_caret_display),
        );
        if accessibility_active {
            let mut nodes = self.state.accessibility.build_shared(
                self.id,
                rect,
                origin,
                self.editor.snapshot().document_id(),
                galley,
                accessible_selection,
            );
            if let Some(projection) = geometry.projection() {
                self.state
                    .accessibility
                    .decorate_rich_nodes(&mut nodes, projection);
            }
            emit_accessibility(ui, nodes);
        }
        if self.state.scroll_to_caret {
            ui.scroll_to_rect(caret, Some(egui::Align::Center));
            self.state.scroll_to_caret = false;
        } else if (surface.corrected_scroll_top - viewport_top).abs() > 0.5 {
            ui.scroll_to_rect(
                Rect::from_min_size(
                    origin + vec2(0.0, surface.corrected_scroll_top),
                    vec2(1.0, 1.0),
                ),
                Some(egui::Align::TOP),
            );
        }
        if self
            .state
            .configuration
            .as_ref()
            .is_none_or(|config| config.features.tables)
            && !self.state.is_composing()
        {
            if response.secondary_clicked()
                && let Some(point) = response.interact_pointer_pos()
            {
                let hit = geometry.hit_test_caret(relative(point, origin));
                if glyphfolio_editor_core::MarkdownTable::at(
                    &self.editor.snapshot().text(),
                    hit.byte,
                )
                .is_some()
                {
                    self.state.input.interrupt();
                    self.state.external_input_boundary = true;
                    let _ = self.editor.set_selection(Selection::caret(hit.byte));
                    let _ = self.state.begin_native_input(self.editor);
                }
            }
            if glyphfolio_editor_core::MarkdownTable::at(
                &self.editor.snapshot().text(),
                self.editor.selection().head,
            )
            .is_some()
            {
                response.context_menu(|ui| {
                    use glyphfolio_editor_core::TableCommand::*;
                    for (label, command) in [
                        ("Add row", AddRow),
                        ("Delete row", DeleteRow),
                        ("Add column", AddColumn),
                        ("Delete column", DeleteColumn),
                        ("Clear cell", ClearCell),
                        (
                            "Align left",
                            Align(glyphfolio_editor_core::ColumnAlignment::Left),
                        ),
                        (
                            "Align center",
                            Align(glyphfolio_editor_core::ColumnAlignment::Center),
                        ),
                        (
                            "Align right",
                            Align(glyphfolio_editor_core::ColumnAlignment::Right),
                        ),
                        ("Paragraph after table", Exit),
                        ("Delete table", DeleteTable),
                    ] {
                        if ui.button(label).clicked() {
                            if let Err(error) = self.state.execute_command(
                                self.editor,
                                EditCommand::Table(command),
                                ui.ctx(),
                            ) {
                                self.state.last_error = Some(error);
                            }
                            ui.close();
                        }
                    }
                });
            }
        }
        response
    }
}

fn accessible_selection(
    geometry: &Geometry,
    mut selection: Selection,
    composition: Option<usize>,
) -> Selection {
    if let Some(at) = composition {
        return Selection::caret(ByteOffset(at));
    }
    selection.anchor = ByteOffset(geometry.to_display(selection.anchor.0, selection.affinity));
    selection.head = ByteOffset(geometry.to_display(selection.head.0, selection.affinity));
    selection
}

fn paint_shaped(
    context: &egui::Context,
    painter: &egui::Painter,
    state: &mut NativeViewState,
    document: &ShapedDocument,
    origin: Pos2,
    color: Color32,
    spans: &[presentation::PaintSpan],
) {
    let configured_palette = state
        .configuration
        .as_ref()
        .map(|config| chrome::ThemePalette::from_mode(context, config.appearance.theme));
    let clip = painter.clip_rect().translate(-origin.to_vec2());
    let visible: Vec<_> = document
        .glyphs
        .iter()
        .filter(|glyph| geometry::rect(glyph.bounds).intersects(clip))
        .collect();
    let live: HashSet<_> = visible.iter().map(|glyph| glyph.image.id).collect();
    state.glyph_textures.retain(|id, _| live.contains(id));
    let mut ordered: Vec<_> = spans.iter().collect();
    ordered.sort_by_key(|span| span.display_range.start);
    let mut maximum = 0;
    let ends: Vec<_> = ordered
        .iter()
        .map(|span| {
            maximum = maximum.max(span.display_range.end);
            maximum
        })
        .collect();
    let mut metrics = NativePaintMetrics {
        considered_glyphs: document.glyphs.len(),
        painted_glyphs: visible.len(),
        ..Default::default()
    };
    for glyph in visible {
        let first = ends.partition_point(|end| *end <= glyph.source_range.start.0);
        let mut tint = color;
        let mut candidates = 0;
        let mut strike = false;
        for span in ordered[first..]
            .iter()
            .take_while(|span| span.display_range.start < glyph.source_range.end.0)
        {
            candidates += 1;
            if span.display_range.end <= glyph.source_range.start.0 {
                continue;
            }
            match span.kind {
                glyphfolio_editor_markdown::StyleKind::Strikethrough => strike = true,
                glyphfolio_editor_markdown::StyleKind::Syntax => tint = tint.gamma_multiply(0.55),
                glyphfolio_editor_markdown::StyleKind::Code => {
                    if let Some(palette) = &configured_palette {
                        let fenced = state
                            .last_surface
                            .as_ref()
                            .and_then(|surface| surface.geometry.projection())
                            .is_some_and(|projection| {
                                let at = projection.blocks().partition_point(|block| {
                                    block.display_range.end.0 <= glyph.source_range.start.0
                                });
                                projection.blocks().get(at).is_some_and(|block| {
                                    matches!(
                                        block.kind,
                                        glyphfolio_editor_markdown::BlockKind::CodeBlock { .. }
                                    )
                                })
                            });
                        tint = if fenced && !palette.is_dark {
                            palette.text
                        } else {
                            palette.code_text
                        };
                    }
                }
                glyphfolio_editor_markdown::StyleKind::Link { .. } => {
                    tint = configured_palette
                        .as_ref()
                        .map_or(Color32::from_rgb(80, 160, 240), |palette| palette.link)
                }
                _ => {}
            }
        }
        metrics.style_candidates += candidates;
        metrics.maximum_style_candidates_per_glyph =
            metrics.maximum_style_candidates_per_glyph.max(candidates);
        if strike {
            let bounds = translated(geometry::rect(glyph.bounds), origin);
            painter.line_segment(
                [bounds.left_center(), bounds.right_center()],
                Stroke::new(1.0, tint),
            );
        }
        let image = &glyph.image;
        let texture = state.glyph_textures.entry(image.id).or_insert_with(|| {
            context.load_texture(
                format!("source-glyph-{}", image.id),
                egui::ColorImage::from_rgba_unmultiplied(
                    [image.width as usize, image.height as usize],
                    &image.rgba,
                ),
                egui::TextureOptions::LINEAR,
            )
        });
        painter.image(
            texture.id(),
            translated(geometry::rect(glyph.bounds), origin),
            Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
            if image.color { Color32::WHITE } else { tint },
        );
    }
    state.paint_metrics = metrics;
}

fn replace_visible(
    editor: &mut Editor,
    state: &mut NativeViewState,
    geometry: &Geometry,
    text: &str,
    origin: TransactionOrigin,
) -> Result<(), String> {
    if !text.is_empty()
        && geometry.projection().is_some()
        && state.render_engine.paragraph_after_table(editor)
    {
        let snapshot = editor.snapshot();
        let source = snapshot.text();
        let at = editor.selection().head;
        let ending = if source.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let prefix = if source[..at.0].ends_with('\n') {
            ending.to_owned()
        } else {
            ending.repeat(2)
        };
        let suffix = if source[at.0..].starts_with(ending) {
            ending
        } else {
            ""
        };
        let pending = state
            .pending_marks
            .take()
            .filter(|pending| pending.valid(editor));
        let (body, caret, keep) = pending.as_ref().map_or_else(
            || (text.to_owned(), text.len(), false),
            |pending| pending.materialize(text),
        );
        let value = format!("{prefix}{body}{suffix}");
        editor
            .apply(
                Transaction::new(&snapshot)
                    .replace(at..at, value)
                    .with_selection(Selection::caret(ByteOffset(at.0 + prefix.len() + caret)))
                    .with_grapheme_selection(Affinity::After)
                    .with_origin(origin),
            )
            .map_err(|error| error.to_string())?;
        if keep && let Some(pending) = pending {
            state.pending_marks = Some(typing::PendingMarks::new(editor, pending.marks));
        }
        return Ok(());
    }
    if state.presentation() == PresentationMode::Rich
        && editor.selection().is_collapsed()
        && !text.is_empty()
        && text.chars().next_back().is_some_and(char::is_whitespace)
        && let Some(projection) = geometry.projection()
        && let Some(styles) = state.render_engine.source_styles(editor)
    {
        let snapshot = editor.snapshot();
        let source = snapshot.text();
        let at = editor.selection().head;
        let display = geometry.to_display(at.0, editor.selection().affinity);
        let endings: Vec<_> = styles
            .iter()
            .filter(|span| {
                span.source_range.start < at
                    && at < span.source_range.end
                    && matches!(
                        span.kind,
                        glyphfolio_editor_markdown::StyleKind::Strong
                            | glyphfolio_editor_markdown::StyleKind::Emphasis
                            | glyphfolio_editor_markdown::StyleKind::Strikethrough
                    )
                    && projection
                        .source_to_display(span.source_range.end, Affinity::Before)
                        .is_ok_and(|end| end.0 == display)
                    && source[at.0..span.source_range.end.0]
                        .bytes()
                        .all(|byte| matches!(byte, b'*' | b'_' | b'~'))
            })
            .collect();
        if let Some(end) = endings.iter().map(|span| span.source_range.end).max() {
            let split = text.trim_end_matches(char::is_whitespace).len();
            let (body, space) = text.split_at(split);
            let marks = endings
                .iter()
                .map(|span| match span.kind {
                    glyphfolio_editor_markdown::StyleKind::Strong => EditCommand::ToggleStrong,
                    glyphfolio_editor_markdown::StyleKind::Emphasis => EditCommand::ToggleEmphasis,
                    _ => EditCommand::ToggleStrikethrough,
                })
                .collect();
            let mut tx = Transaction::new(&snapshot)
                .with_origin(origin)
                .with_selection(Selection::caret(ByteOffset(end.0 + text.len())))
                .with_grapheme_selection(Affinity::After);
            if !body.is_empty() {
                tx = tx.replace(at..at, body);
            }
            tx = tx.replace(end..end, space);
            editor.apply(tx).map_err(|error| error.to_string())?;
            state.pending_marks = Some(typing::PendingMarks::new(editor, marks));
            return Ok(());
        }
    }
    if state.presentation() == PresentationMode::Rich
        && !text.is_empty()
        && let Some(pending) = state
            .pending_marks
            .take()
            .filter(|pending| pending.valid(editor))
    {
        let (source, caret, keep) = pending.materialize(text);
        let snapshot = editor.snapshot();
        let at = editor.selection().head;
        editor
            .apply(
                Transaction::new(&snapshot)
                    .replace(at..at, source)
                    .with_selection(Selection::caret(ByteOffset(at.0 + caret)))
                    .with_grapheme_selection(Affinity::After)
                    .with_origin(origin),
            )
            .map_err(|error| error.to_string())?;
        if keep {
            state.pending_marks = Some(typing::PendingMarks::new(editor, pending.marks));
        }
        return Ok(());
    }
    if let Some(projection) = geometry.projection()
        && let Some(styles) = state.render_engine.source_styles(editor)
    {
        let selected = editor.selection();
        let start = geometry.to_display(selected.start().0, Affinity::After);
        let end = geometry.to_display(
            selected.end().0,
            if selected.is_collapsed() {
                Affinity::After
            } else {
                Affinity::Before
            },
        );
        return rich_edit::replace(
            editor,
            projection,
            styles,
            start..end.max(start),
            text,
            origin,
        );
    }
    editor
        .insert_text(text, origin)
        .map(|_| ())
        .map_err(|error| error.to_string())
}
fn delete_visible(
    editor: &mut Editor,
    state: &mut NativeViewState,
    geometry: &Geometry,
    backward: bool,
) -> Result<(), String> {
    let Some(projection) = geometry.projection() else {
        return if backward {
            editor
                .execute(EditCommand::Backspace)
                .map(|_| ())
                .map_err(|error| error.to_string())
        } else {
            editor
                .delete_forward()
                .map(|_| ())
                .map_err(|error| error.to_string())
        };
    };
    let selected = editor.selection();
    let mut start = geometry.to_display(selected.start().0, Affinity::After);
    let mut end = geometry.to_display(
        selected.end().0,
        if selected.is_collapsed() {
            Affinity::After
        } else {
            Affinity::Before
        },
    );
    if selected.is_collapsed() {
        if let Some(table) =
            glyphfolio_editor_core::MarkdownTable::at(&editor.snapshot().text(), selected.head)
        {
            let (r, c) = table.cell_at(selected.head);
            let cell = &table.rows[r].cells[c];
            if backward && selected.head <= cell.start || !backward && selected.head >= cell.end {
                return Ok(());
            }
        }
        if backward {
            if let Some(block) = projection
                .blocks()
                .iter()
                .find(|block| block.display_range.start.0 == start)
            {
                let command = if let glyphfolio_editor_markdown::BlockKind::Heading(_) = block.kind
                {
                    Some(EditCommand::SetHeading(None))
                } else if matches!(
                    block.kind,
                    glyphfolio_editor_markdown::BlockKind::CodeBlock { .. }
                ) {
                    Some(EditCommand::ToggleCodeFence)
                } else if block.context.list_item_start {
                    match block.context.list_kind {
                        Some(glyphfolio_editor_markdown::ListKind::Unordered) => {
                            Some(EditCommand::ToggleUnorderedList)
                        }
                        Some(glyphfolio_editor_markdown::ListKind::Ordered { .. }) => {
                            Some(EditCommand::ToggleOrderedList)
                        }
                        _ => None,
                    }
                } else if block.context.quote_depth > 0 {
                    Some(EditCommand::ToggleBlockQuote)
                } else {
                    None
                };
                if let Some(command) = command {
                    return state.run_edit_command(editor, command);
                }
                if let Some(styles) = state.render_engine.source_styles(editor)
                    && rich_edit::join_backward(editor, projection, styles)?.is_some()
                {
                    return Ok(());
                }
            }
            start = projection.text()[..start]
                .grapheme_indices(true)
                .next_back()
                .map_or(start, |(offset, _)| offset);
        } else {
            end = projection.text()[end..]
                .graphemes(true)
                .next()
                .map_or(end, |text| end + text.len());
        }
    }
    if start == end {
        return Ok(());
    }
    let styles = state
        .render_engine
        .source_styles(editor)
        .ok_or("Rich projection is stale")?;
    rich_edit::replace(
        editor,
        projection,
        styles,
        start..end,
        "",
        TransactionOrigin::Delete,
    )
}

fn table_input(editor: &Editor, input: &str, mode: PresentationMode) -> String {
    if mode != PresentationMode::Rich {
        return input.into();
    }
    let source = editor.snapshot().text();
    let selected = editor.selection();
    let Some(table) = glyphfolio_editor_core::MarkdownTable::at(&source, selected.head) else {
        return input.into();
    };
    let in_cell = table
        .rows
        .iter()
        .flat_map(|row| &row.cells)
        .any(|cell| cell.start <= selected.start() && selected.end() <= cell.end);
    if !in_cell {
        return input.into();
    }
    let mut slash = source[..selected.start().0]
        .bytes()
        .rev()
        .take_while(|byte| *byte == b'\\')
        .count();
    let mut result = String::new();
    for ch in input.replace("\r\n", "\n").replace('\r', "\n").chars() {
        if ch == '\n' {
            result.push_str("<br>");
            slash = 0;
            continue;
        }
        if ch == '|' && slash % 2 == 0 {
            result.push('\\');
        }
        result.push(ch);
        slash = if ch == '\\' { slash + 1 } else { 0 };
    }
    result
}

fn handled_key(key: Key) -> bool {
    matches!(
        key,
        Key::ArrowLeft
            | Key::ArrowRight
            | Key::ArrowUp
            | Key::ArrowDown
            | Key::Home
            | Key::End
            | Key::Backspace
            | Key::Delete
            | Key::Enter
            | Key::Tab
            | Key::Escape
            | Key::A
            | Key::Z
            | Key::Y
            | Key::B
            | Key::I
            | Key::E
            | Key::PageUp
            | Key::PageDown
    )
}

fn key_command(
    editor: &mut Editor,
    state: &mut NativeViewState,
    key: Key,
    modifiers: Modifiers,
    geometry: &Geometry,
) -> Result<(), String> {
    if matches!(
        key,
        Key::ArrowLeft
            | Key::ArrowRight
            | Key::ArrowUp
            | Key::ArrowDown
            | Key::Home
            | Key::End
            | Key::PageUp
            | Key::PageDown
            | Key::Enter
            | Key::Tab
            | Key::Escape
            | Key::Backspace
            | Key::Delete
    ) {
        state.pending_marks = None;
    }
    if modifiers.command && matches!(key, Key::Z | Key::Y) && !state.history_allowed() {
        return Err("Undo and redo are disabled by the editor configuration".into());
    }
    if modifiers.command {
        match key {
            Key::A => {
                let mut selected = editor.selection();
                selected.anchor = ByteOffset(0);
                selected.head = ByteOffset(editor.snapshot().byte_len());
                return editor.set_selection(selected).map_err(|e| e.to_string());
            }
            Key::Z if modifiers.shift => {
                return editor.redo().map(|_| ()).map_err(|e| e.to_string());
            }
            Key::Z => return editor.undo().map(|_| ()).map_err(|e| e.to_string()),
            Key::Y => return editor.redo().map(|_| ()).map_err(|e| e.to_string()),
            Key::B => return state.run_edit_command(editor, EditCommand::ToggleStrong),
            Key::I => return state.run_edit_command(editor, EditCommand::ToggleEmphasis),
            Key::E => return state.run_edit_command(editor, EditCommand::ToggleInlineCode),
            _ => {}
        }
    }
    let mut selected = editor.selection();
    match key {
        Key::Home | Key::End if modifiers.command => {
            selected.head = ByteOffset(if key == Key::Home {
                0
            } else {
                editor.snapshot().byte_len()
            });
            state.preferred_x = None;
        }
        Key::ArrowUp | Key::ArrowDown if modifiers.mac_cmd => {
            selected.head = ByteOffset(if key == Key::ArrowUp {
                0
            } else {
                editor.snapshot().byte_len()
            });
            state.preferred_x = None;
        }
        Key::ArrowLeft | Key::ArrowRight
            if !modifiers.command && !modifiers.alt && !modifiers.ctrl =>
        {
            state.preferred_x = None;
            let right = key == Key::ArrowRight;
            let caret = if !modifiers.shift && !selected.is_collapsed() {
                // Collapse to the visual endpoint without moving an extra
                // grapheme. A pure RTL selection reverses its source endpoints.
                let start =
                    geometry.caret(selected.start(), glyphfolio_editor_core::Affinity::After);
                let end = geometry.caret(selected.end(), glyphfolio_editor_core::Affinity::Before);
                let order = start.y.total_cmp(&end.y).then(start.x.total_cmp(&end.x));
                if order.is_lt() == right { end } else { start }
            } else {
                geometry.visual_neighbor(selected.head, selected.affinity, right)
            };
            selected.head = caret.byte;
            selected.affinity = caret.affinity;
        }
        Key::Home | Key::End => {
            let caret = geometry.row_edge(selected.head, selected.affinity, key == Key::End);
            selected.head = caret.byte;
            selected.affinity = caret.affinity;
            state.preferred_x = None;
        }
        Key::ArrowLeft | Key::ArrowRight if modifiers.mac_cmd => {
            let caret = geometry.row_edge(selected.head, selected.affinity, key == Key::ArrowRight);
            selected.head = caret.byte;
            selected.affinity = caret.affinity;
            state.preferred_x = None;
        }
        Key::ArrowLeft | Key::ArrowRight => {
            selected.head = if let Some(projection) = geometry.projection() {
                let at = geometry.to_display(selected.head.0, selected.affinity);
                let target = word_target(projection.text(), at, key == Key::ArrowRight);
                ByteOffset(geometry.to_source(
                    target,
                    if key == Key::ArrowRight {
                        Affinity::Before
                    } else {
                        Affinity::After
                    },
                ))
            } else {
                ByteOffset(word_target(
                    &editor.snapshot().text(),
                    selected.head.0,
                    key == Key::ArrowRight,
                ))
            };
            state.preferred_x = None;
        }
        Key::ArrowUp | Key::ArrowDown => {
            let x = *state.preferred_x.get_or_insert_with(|| {
                geometry
                    .caret_with_affinity(selected.head.0, selected.affinity)
                    .left()
            });
            let caret = geometry.vertical_target(
                selected.head,
                selected.affinity,
                key == Key::ArrowDown,
                x,
            );
            if key == Key::ArrowDown
                && !modifiers.shift
                && selected.is_collapsed()
                && let Some(table) = glyphfolio_editor_core::MarkdownTable::at(
                    &editor.snapshot().text(),
                    selected.head,
                )
                && (caret.byte == selected.head || caret.byte >= table.range.end)
                && state.command_allowed(EditCommand::Table(
                    glyphfolio_editor_core::TableCommand::Exit,
                ))
            {
                return state.run_edit_command(
                    editor,
                    EditCommand::Table(glyphfolio_editor_core::TableCommand::Exit),
                );
            }
            selected.head = caret.byte;
            selected.affinity = caret.affinity;
        }
        Key::PageUp | Key::PageDown => {
            let current = geometry.caret(selected.head, selected.affinity);
            let x = *state.preferred_x.get_or_insert(current.x);
            let target_y = if key == Key::PageDown {
                current.y + state.viewport_height
            } else {
                current.y - state.viewport_height
            };
            let hit = geometry.hit_test_caret(egui::pos2(x, target_y.max(0.0)));
            selected.head = hit.byte;
            selected.affinity = hit.affinity;
        }
        Key::Backspace => return delete_visible(editor, state, geometry, true),
        Key::Delete => return delete_visible(editor, state, geometry, false),
        Key::Enter
            if state.command_allowed(EditCommand::Table(
                glyphfolio_editor_core::TableCommand::AddRow,
            )) && state.presentation() == PresentationMode::Rich
                && glyphfolio_editor_core::MarkdownTable::at(
                    &editor.snapshot().text(),
                    editor.selection().head,
                )
                .is_some() =>
        {
            if modifiers.command || state.render_engine.paragraph_after_table(editor) {
                return state.run_edit_command(
                    editor,
                    EditCommand::Table(glyphfolio_editor_core::TableCommand::Exit),
                );
            }
            if modifiers.shift {
                let text = table_input(editor, "\n", PresentationMode::Rich);
                return replace_visible(editor, state, geometry, &text, TransactionOrigin::Typing);
            }
            return state.run_edit_command(
                editor,
                EditCommand::Table(glyphfolio_editor_core::TableCommand::NextRow),
            );
        }
        Key::Enter if geometry.projection().is_some() => {
            let marks = rich_edit::paragraph_break(
                editor,
                geometry.projection().unwrap(),
                modifiers.shift,
            )?;
            state.pending_marks =
                (!marks.is_empty()).then(|| typing::PendingMarks::new(editor, marks));
            return Ok(());
        }
        Key::Enter => {
            return editor
                .execute(EditCommand::Enter)
                .map(|_| ())
                .map_err(|e| e.to_string());
        }
        Key::Tab
            if state.command_allowed(EditCommand::Table(
                glyphfolio_editor_core::TableCommand::NextCell,
            )) && glyphfolio_editor_core::MarkdownTable::at(
                &editor.snapshot().text(),
                editor.selection().head,
            )
            .is_some() =>
        {
            return state.run_edit_command(
                editor,
                EditCommand::Table(if modifiers.shift {
                    glyphfolio_editor_core::TableCommand::PreviousCell
                } else {
                    glyphfolio_editor_core::TableCommand::NextCell
                }),
            );
        }
        Key::Tab => {
            state.check_command(if modifiers.shift {
                EditCommand::Outdent
            } else {
                EditCommand::Indent
            })?;
            return editor
                .execute(if modifiers.shift {
                    EditCommand::Outdent
                } else {
                    EditCommand::Indent
                })
                .map(|_| ())
                .map_err(|e| e.to_string());
        }
        _ => return Ok(()),
    }
    if !modifiers.shift {
        selected.anchor = selected.head;
    }
    editor.set_selection(selected).map_err(|e| e.to_string())
}

fn word_selection_range(text: &str, at: usize) -> std::ops::Range<usize> {
    text.split_word_bound_indices()
        .find(|(start, value)| *start <= at && at < start + value.len())
        .or_else(|| text.split_word_bound_indices().next_back())
        .map_or(0..0, |(start, value)| start..start + value.len())
}

fn logical_line_range(text: &str, at: usize) -> std::ops::Range<usize> {
    let at = at.min(text.len());
    let start = text[..at].rfind(['\r', '\n']).map_or(0, |index| index + 1);
    let end = text[at..]
        .find(['\r', '\n'])
        .map_or(text.len(), |relative| {
            let newline = at + relative;
            newline
                + if text.as_bytes()[newline..].starts_with(b"\r\n") {
                    2
                } else {
                    1
                }
        });
    start..end
}

fn word_target(text: &str, at: usize, forward: bool) -> usize {
    if forward {
        text.split_word_bound_indices()
            .filter(|(_, word)| !word.chars().all(char::is_whitespace))
            .map(|(offset, word)| offset + word.len())
            .find(|offset| *offset > at)
            .unwrap_or(text.len())
    } else {
        text.split_word_bound_indices()
            .filter(|(_, word)| !word.chars().all(char::is_whitespace))
            .map(|(offset, _)| offset)
            .take_while(|offset| *offset < at)
            .last()
            .unwrap_or(0)
    }
}
fn accessibility_action(
    editor: &mut Editor,
    state: &mut NativeViewState,
    request: &accesskit::ActionRequest,
    id: Id,
    geometry: &Geometry,
    text: &str,
) -> Result<(), String> {
    match (&request.action, &request.data) {
        (accesskit::Action::Focus, _) => {
            state.request_focus();
            Ok(())
        }
        (
            accesskit::Action::SetTextSelection,
            Some(accesskit::ActionData::SetTextSelection(selected)),
        ) => {
            let anchor = state
                .accessibility
                .source_position(selected.anchor, text)
                .ok_or("Unknown accessible anchor")?
                .0;
            let head = state
                .accessibility
                .source_position(selected.focus, text)
                .ok_or("Unknown accessible focus")?
                .0;
            let legal =
                |byte| byte == text.len() || text.grapheme_indices(true).any(|(at, _)| at == byte);
            if !legal(anchor) || !legal(head) {
                return Err("Accessible selection must use grapheme boundaries".into());
            }
            if let Some(projection) = geometry.projection() {
                // Mapping affinity cannot authorize a partial entity/code-span
                // replacement. Use the projection's editability contract even
                // though this action changes selection rather than source.
                let _ = projection
                    .projected_edit(
                        &editor.snapshot(),
                        glyphfolio_editor_markdown::DisplayOffset(anchor.min(head))
                            ..glyphfolio_editor_markdown::DisplayOffset(anchor.max(head)),
                        "",
                        glyphfolio_editor_core::Affinity::After,
                        TransactionOrigin::Programmatic,
                    )
                    .map_err(|error| error.to_string())?;
            }
            let mut selection = editor.selection();
            use glyphfolio_editor_core::Affinity::{After, Before};
            let (anchor_affinity, head_affinity) = if anchor < head {
                (After, Before)
            } else if anchor > head {
                (Before, After)
            } else {
                (After, After)
            };
            selection.anchor = ByteOffset(geometry.to_source(anchor, anchor_affinity));
            selection.head = ByteOffset(geometry.to_source(head, head_affinity));
            selection.affinity = head_affinity;
            editor.set_selection(selection).map_err(|e| e.to_string())
        }
        (accesskit::Action::ReplaceSelectedText, Some(accesskit::ActionData::Value(value)))
            if request.target_node == id.accesskit_id() =>
        {
            editor
                .insert_text(value, TransactionOrigin::Programmatic)
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
        (accesskit::Action::SetValue, Some(accesskit::ActionData::Value(value)))
            if request.target_node == id.accesskit_id() =>
        {
            let snapshot = editor.snapshot();
            editor
                .apply(
                    Transaction::new(&snapshot)
                        .replace(
                            ByteOffset(0)..ByteOffset(snapshot.byte_len()),
                            value.as_ref(),
                        )
                        .with_selection(Selection::caret(ByteOffset(value.len()))),
                )
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
        _ => Ok(()),
    }
}

fn emit_accessibility(ui: &Ui, nodes: Vec<(Id, accesskit::Node)>) {
    let descendants: HashSet<_> = nodes
        .iter()
        .skip(1)
        .map(|(id, _)| id.accesskit_id())
        .collect();
    for (id, node) in nodes {
        ui.ctx()
            .accesskit_node_builder(id, |builder| *builder = node);
    }
    ui.ctx()
        .accesskit_node_builder(egui::accesskit_root_id(), |root| {
            root.set_children(
                root.children()
                    .iter()
                    .copied()
                    .filter(|id| !descendants.contains(id))
                    .collect::<Vec<_>>(),
            );
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn geometry_for(text: &str) -> Geometry {
        let document = ShapingEngine::new().shape(
            text,
            LayoutOptions {
                width: 240.0,
                ..Default::default()
            },
        );
        Geometry::from_shaped(Id::unique("test"), Arc::new(document))
    }
    #[test]
    fn composition_provisional_cancel_safe_and_one_history_step() {
        let mut editor = Editor::new("before");
        editor
            .set_selection(Selection::caret(ByteOffset(6)))
            .unwrap();
        let mut state = NativeViewState::default();
        state.refresh_text(&editor);
        state.preedit(&editor, "にほん", Some(0..3));
        assert_eq!(editor.snapshot().text(), "before");
        assert!(!state.can_close());
        state.request_presentation(PresentationMode::Rich);
        assert_eq!(state.presentation(), PresentationMode::Source);
        state.commit(&mut editor, "日本").unwrap();
        assert_eq!(editor.snapshot().text(), "before日本");
        assert_eq!(state.presentation(), PresentationMode::Rich);
        editor.undo().unwrap();
        assert_eq!(editor.snapshot().text(), "before");
        state.begin_native_input(&editor).unwrap();
        state.preedit(&editor, "cancel", None);
        state.cancel_composition();
        assert_eq!(editor.snapshot().text(), "before");
    }
    #[test]
    fn stale_composition_cannot_mutate_changed_document() {
        let mut editor = Editor::new("source");
        let mut state = NativeViewState::default();
        state.preedit(&editor, "pending", None);
        editor
            .insert_text("external", TransactionOrigin::Programmatic)
            .unwrap();
        let before = editor.snapshot().text();
        assert!(state.commit(&mut editor, "late").is_err());
        assert_eq!(editor.snapshot().text(), before);
    }
    fn route(
        context: &egui::Context,
        editor: &mut Editor,
        state: &mut NativeViewState,
        events: Vec<Event>,
        focused: bool,
    ) -> egui::FullOutput {
        let mut output = context.run_ui(
            egui::RawInput {
                events,
                focused,
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(640.0, 480.0))),
                ..Default::default()
            },
            |ui| {
                ui.add(EditorWidget::new(editor, state).id(Id::unique("route")));
            },
        );
        output.textures_delta.clear();
        output
    }
    fn route_with_host_controls(
        context: &egui::Context,
        editor: &mut Editor,
        state: &mut NativeViewState,
        events: Vec<Event>,
    ) -> egui::FullOutput {
        let mut output = context.run_ui(
            egui::RawInput {
                events,
                focused: true,
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(640.0, 640.0))),
                ..Default::default()
            },
            |ui| {
                // Native host controls in each direction would receive ordinary
                // egui arrow-focus navigation if the editor's filter were absent.
                for (label, position) in [
                    ("top", egui::pos2(220.0, 20.0)),
                    ("left", egui::pos2(10.0, 230.0)),
                    ("right", egui::pos2(480.0, 230.0)),
                    ("bottom", egui::pos2(220.0, 440.0)),
                ] {
                    let _ = ui.put(
                        Rect::from_min_size(position, vec2(90.0, 30.0)),
                        egui::Button::new(label),
                    );
                }
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(Rect::from_min_size(
                        egui::pos2(140.0, 100.0),
                        vec2(300.0, 280.0),
                    )),
                    |ui| {
                        ui.add(EditorWidget::new(editor, state).id(Id::unique("route")));
                    },
                );
            },
        );
        output.textures_delta.clear();
        output
    }
    #[test]
    fn native_event_route_keeps_preedit_provisional_and_positions_candidates() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut editor = Editor::new("start");
        editor
            .set_selection(Selection::caret(ByteOffset(5)))
            .unwrap();
        let mut state = NativeViewState::default();
        state.request_focus();
        let _ = route(&context, &mut editor, &mut state, vec![], true);
        let output = route(
            &context,
            &mut editor,
            &mut state,
            vec![
                Event::Ime(egui::ImeEvent::Preedit {
                    text: "にほん".into(),
                    active_range_chars: Some(0..3),
                }),
                Event::Text("provisional".into()),
            ],
            true,
        );
        assert!(state.is_composing());
        assert_eq!(editor.snapshot().text(), "start");
        let candidate = output
            .platform_output
            .ime
            .expect("focused surface emits candidate geometry");
        assert!(candidate.cursor_rect.height() > 0.0);
        assert_eq!(Some(candidate.cursor_rect), state.candidate_rect);
        let tree = output
            .platform_output
            .accesskit_update
            .expect("editable accessibility tree");
        let editor_id = Id::unique("route").accesskit_id();
        let node = &tree
            .nodes
            .iter()
            .find(|(id, _)| *id == editor_id)
            .unwrap()
            .1;
        assert_eq!(node.role(), accesskit::Role::MultilineTextInput);
        for child in node.children() {
            assert_eq!(
                tree.nodes
                    .iter()
                    .filter(|(_, node)| node.children().contains(child))
                    .count(),
                1,
                "each text run has exactly one parent"
            );
        }
        state.request_presentation(PresentationMode::Rich);
        let _ = route(
            &context,
            &mut editor,
            &mut state,
            vec![
                Event::Ime(egui::ImeEvent::Preedit {
                    text: String::new(),
                    active_range_chars: None,
                }),
                Event::Ime(egui::ImeEvent::Commit("日本".into())),
            ],
            true,
        );
        assert_eq!(editor.snapshot().text(), "start日本");
        assert!(!state.is_composing());
        assert_eq!(state.presentation(), PresentationMode::Rich);
        editor.undo().unwrap();
        assert_eq!(editor.snapshot().text(), "start");
    }
    #[test]
    fn navigation_and_escape_keep_editor_focus_across_frames() {
        let context = egui::Context::default();
        let id = Id::unique("route");
        let mut editor = Editor::new("abc\ndef");
        editor
            .set_selection(Selection::caret(ByteOffset(2)))
            .unwrap();
        let mut state = NativeViewState::default();
        state.request_focus();
        // Focus is established in one pass; egui installs its lock filter once
        // the same widget had focus during the preceding pass.
        let _ = route_with_host_controls(&context, &mut editor, &mut state, vec![]);
        let _ = route_with_host_controls(&context, &mut editor, &mut state, vec![]);
        for key in [
            Key::ArrowRight,
            Key::ArrowLeft,
            Key::ArrowUp,
            Key::ArrowDown,
            Key::Escape,
        ] {
            let _ = route_with_host_controls(
                &context,
                &mut editor,
                &mut state,
                vec![Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                }],
            );
            if key == Key::ArrowRight {
                assert_eq!(editor.selection().head, ByteOffset(3));
            } else if key == Key::ArrowLeft {
                assert_eq!(editor.selection().head, ByteOffset(2));
            }
            assert!(
                context.memory(|memory| memory.has_focus(id)),
                "lost focus on {key:?}"
            );
            let output = route_with_host_controls(&context, &mut editor, &mut state, vec![]);
            assert!(
                context.memory(|memory| memory.has_focus(id)),
                "lost focus after {key:?}"
            );
            assert!(
                output.platform_output.ime.is_some(),
                "native input owner lost after {key:?}"
            );
        }
        assert_eq!(editor.snapshot().text(), "abc\ndef");
        assert!(!editor.can_undo());
    }
    #[test]
    fn composing_escape_cancels_without_surrendering_editor_focus() {
        let context = egui::Context::default();
        let id = Id::unique("route");
        let mut editor = Editor::new("safe");
        let mut state = NativeViewState::default();
        state.request_focus();
        let _ = route(&context, &mut editor, &mut state, vec![], true);
        let _ = route(&context, &mut editor, &mut state, vec![], true);
        let _ = route(
            &context,
            &mut editor,
            &mut state,
            vec![Event::Ime(egui::ImeEvent::Preedit {
                text: "にほん".into(),
                active_range_chars: Some(0..3),
            })],
            true,
        );
        assert!(state.is_composing());
        assert!(!state.can_close());
        let _ = route(
            &context,
            &mut editor,
            &mut state,
            vec![Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            true,
        );
        assert!(!state.is_composing());
        assert!(state.can_close());
        assert!(context.memory(|memory| memory.has_focus(id)));
        let _ = route(&context, &mut editor, &mut state, vec![], true);
        assert!(context.memory(|memory| memory.has_focus(id)));
        assert_eq!(editor.snapshot().text(), "safe");
        assert!(!editor.can_undo());
        // Ordinary typing still reaches the same surface after cancelling.
        let _ = route(
            &context,
            &mut editor,
            &mut state,
            vec![Event::Text("x".into())],
            true,
        );
        assert_eq!(editor.snapshot().text(), "xsafe");
    }
    #[test]
    fn window_focus_loss_cancels_and_rejects_late_commit() {
        let context = egui::Context::default();
        let mut editor = Editor::new("safe");
        let mut state = NativeViewState::default();
        state.request_focus();
        let _ = route(&context, &mut editor, &mut state, vec![], true);
        let _ = route(
            &context,
            &mut editor,
            &mut state,
            vec![Event::Ime(egui::ImeEvent::Preedit {
                text: "pending".into(),
                active_range_chars: None,
            })],
            true,
        );
        let _ = route(
            &context,
            &mut editor,
            &mut state,
            vec![Event::WindowFocused(false)],
            false,
        );
        assert!(!state.is_composing());
        assert_eq!(editor.snapshot().text(), "safe");
        state.request_focus();
        let _ = route(
            &context,
            &mut editor,
            &mut state,
            vec![Event::Ime(egui::ImeEvent::Commit("late".into()))],
            true,
        );
        assert_eq!(editor.snapshot().text(), "safe");
    }
    #[test]
    fn dismissed_preedit_keeps_session_guard_after_host_document_replacement() {
        let context = egui::Context::default();
        let mut editor = Editor::new("document A");
        let mut state = NativeViewState::default();
        state.request_focus();
        let _ = route(&context, &mut editor, &mut state, vec![], true);
        let _ = route(
            &context,
            &mut editor,
            &mut state,
            vec![Event::Ime(egui::ImeEvent::Preedit {
                text: "pending".into(),
                active_range_chars: None,
            })],
            true,
        );
        editor = Editor::new("document B");
        let _ = route(
            &context,
            &mut editor,
            &mut state,
            vec![
                Event::Ime(egui::ImeEvent::Preedit {
                    text: String::new(),
                    active_range_chars: None,
                }),
                Event::Ime(egui::ImeEvent::Commit("wrong document".into())),
            ],
            true,
        );
        assert_eq!(editor.snapshot().text(), "document B");
        assert!(!state.is_composing());
    }
    #[test]
    fn dismissed_preedit_rejects_same_revision_selection_change() {
        let context = egui::Context::default();
        let mut editor = Editor::new("abcdef");
        let mut selected = Selection::caret(ByteOffset(1));
        selected.head = ByteOffset(3);
        editor.set_selection(selected).unwrap();
        let original_revision = editor.snapshot().revision();
        let mut state = NativeViewState::default();
        state.request_focus();
        let _ = route(&context, &mut editor, &mut state, vec![], true);
        let _ = route(
            &context,
            &mut editor,
            &mut state,
            vec![Event::Ime(egui::ImeEvent::Preedit {
                text: "仮".into(),
                active_range_chars: None,
            })],
            true,
        );
        let _ = route(
            &context,
            &mut editor,
            &mut state,
            vec![Event::Ime(egui::ImeEvent::Preedit {
                text: String::new(),
                active_range_chars: None,
            })],
            true,
        );
        // Selection-only host activity does not increment the source revision.
        let moved = Selection::caret(ByteOffset(6));
        editor.set_selection(moved).unwrap();
        assert_eq!(editor.snapshot().revision(), original_revision);
        let _ = route(
            &context,
            &mut editor,
            &mut state,
            vec![Event::Ime(egui::ImeEvent::Commit("完成".into()))],
            true,
        );
        assert_eq!(editor.snapshot().text(), "abcdef");
        assert_eq!(editor.selection(), moved);
        assert_eq!(editor.snapshot().revision(), original_revision);
        assert!(!editor.can_undo());
        assert!(state.last_error.is_some());
    }
    #[test]
    fn native_duplicate_suppression_consumes_only_one_expected_event() {
        let context = egui::Context::default();
        let mut editor = Editor::new("");
        let mut state = NativeViewState::default();
        state.request_focus();
        let _ = route(&context, &mut editor, &mut state, vec![], true);
        let _ = route(
            &context,
            &mut editor,
            &mut state,
            vec![
                Event::Ime(egui::ImeEvent::Commit("a".into())),
                Event::Text("a".into()),
                Event::Text("a".into()),
            ],
            true,
        );
        assert_eq!(editor.snapshot().text(), "aa");
    }
    #[test]
    fn native_paste_preserves_raw_crlf_without_overriding_another_payload() {
        struct RawClipboard(String);
        impl clipboard::ClipboardService for RawClipboard {
            fn read_text(&mut self) -> Result<String, String> {
                Ok(self.0.clone())
            }
            fn write_text(&mut self, value: &str) -> Result<(), String> {
                self.0 = value.into();
                Ok(())
            }
        }
        let raw = "**日本 😀**\r\nline\r\n";
        for (event, expected) in [
            (raw.replace("\r\n", "\n"), raw),
            ("host payload".into(), "host payload"),
        ] {
            let context = egui::Context::default();
            let mut editor = Editor::new("left TARGET right");
            editor
                .set_selection(Selection {
                    anchor: ByteOffset(5),
                    head: ByteOffset(11),
                    ..Selection::default()
                })
                .unwrap();
            let mut state = NativeViewState::default();
            state.set_clipboard(Box::new(RawClipboard(raw.into())));
            state.request_focus();
            let _ = route(&context, &mut editor, &mut state, vec![], true);
            let _ = route(
                &context,
                &mut editor,
                &mut state,
                vec![Event::Paste(event)],
                true,
            );
            assert_eq!(editor.snapshot().text(), format!("left {expected} right"));
            editor.undo().unwrap();
            assert_eq!(editor.snapshot().text(), "left TARGET right");
            assert!(!editor.can_undo());
        }
    }
    #[test]
    fn intervening_native_events_end_commit_duplicate_suppression() {
        let key = |pressed| Event::Key {
            key: Key::F1,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        for boundary in [Event::Copy, key(true), key(false)] {
            let context = egui::Context::default();
            let mut editor = Editor::new("");
            let mut state = NativeViewState::default();
            state.request_focus();
            let _ = route(&context, &mut editor, &mut state, vec![], true);
            let _ = route(
                &context,
                &mut editor,
                &mut state,
                vec![
                    Event::Ime(egui::ImeEvent::Commit("a".into())),
                    boundary,
                    Event::Text("a".into()),
                ],
                true,
            );
            assert_eq!(editor.snapshot().text(), "aa");
        }
    }
    #[test]
    fn horizontal_arrows_collapse_selections_without_an_extra_step() {
        let geometry = geometry_for("abcdef");
        let mut state = NativeViewState::default();
        for (anchor, head) in [(1, 4), (4, 1)] {
            for (key, expected) in [(Key::ArrowLeft, 1), (Key::ArrowRight, 4)] {
                let mut editor = Editor::new("abcdef");
                editor
                    .set_selection(Selection {
                        anchor: ByteOffset(anchor),
                        head: ByteOffset(head),
                        ..Selection::default()
                    })
                    .unwrap();
                key_command(&mut editor, &mut state, key, Modifiers::NONE, &geometry).unwrap();
                assert!(editor.selection().is_collapsed());
                assert_eq!(editor.selection().head, ByteOffset(expected));
            }
        }
        let text = "אבג";
        let geometry = geometry_for(text);
        for (key, expected) in [(Key::ArrowLeft, text.len()), (Key::ArrowRight, 0)] {
            let mut editor = Editor::new(text);
            editor
                .set_selection(Selection {
                    anchor: ByteOffset(0),
                    head: ByteOffset(text.len()),
                    ..Selection::default()
                })
                .unwrap();
            key_command(&mut editor, &mut state, key, Modifiers::NONE, &geometry).unwrap();
            assert!(editor.selection().is_collapsed());
            assert_eq!(editor.selection().head, ByteOffset(expected));
        }
    }
    #[test]
    fn word_navigation_skips_whitespace_and_keeps_combining_text_intact() {
        let text = "alpha   e\u{301} 日本";
        let geometry = geometry_for(text);
        let mut state = NativeViewState::default();
        let mut editor = Editor::new(text);
        let modifiers = Modifiers {
            alt: true,
            ..Modifiers::NONE
        };
        for expected in [5, 11, 15, 18] {
            key_command(
                &mut editor,
                &mut state,
                Key::ArrowRight,
                modifiers,
                &geometry,
            )
            .unwrap();
            assert_eq!(editor.selection().head, ByteOffset(expected));
        }
        for expected in [15, 12, 8, 0] {
            key_command(
                &mut editor,
                &mut state,
                Key::ArrowLeft,
                modifiers,
                &geometry,
            )
            .unwrap();
            assert_eq!(editor.selection().head, ByteOffset(expected));
        }
        assert_eq!(editor.snapshot().text(), text);
        assert!(!editor.can_undo());
    }
    #[test]
    fn source_shortcuts_format_indent_and_continue_lists_with_shared_undo() {
        let context = egui::Context::default();
        let mut editor = Editor::new("日本");
        editor
            .set_selection(Selection {
                anchor: ByteOffset(0),
                head: ByteOffset(6),
                ..Selection::default()
            })
            .unwrap();
        let mut state = NativeViewState::default();
        state.request_focus();
        let _ = route(&context, &mut editor, &mut state, vec![], true);
        let shortcut = |key| Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers {
                command: true,
                ..Modifiers::NONE
            },
        };
        let _ = route(
            &context,
            &mut editor,
            &mut state,
            vec![shortcut(Key::B)],
            true,
        );
        assert_eq!(editor.snapshot().text(), "**日本**");
        let _ = route(
            &context,
            &mut editor,
            &mut state,
            vec![shortcut(Key::Z)],
            true,
        );
        assert_eq!(editor.snapshot().text(), "日本");
        assert!(!editor.can_undo());

        editor = Editor::new("- alpha");
        editor
            .set_selection(Selection::caret(ByteOffset(7)))
            .unwrap();
        state.cancel_composition();
        let _ = route(&context, &mut editor, &mut state, vec![], true);
        let key = |key, shift| Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers {
                shift,
                ..Modifiers::NONE
            },
        };
        let _ = route(
            &context,
            &mut editor,
            &mut state,
            vec![key(Key::Enter, false)],
            true,
        );
        assert_eq!(editor.snapshot().text(), "- alpha\n- ");
        let _ = route(
            &context,
            &mut editor,
            &mut state,
            vec![key(Key::Tab, false)],
            true,
        );
        assert!(editor.snapshot().text().ends_with("  - "));
        assert!(context.memory(|memory| memory.has_focus(Id::unique("route"))));
        let _ = route(
            &context,
            &mut editor,
            &mut state,
            vec![key(Key::Tab, true)],
            true,
        );
        assert_eq!(editor.snapshot().text(), "- alpha\n- ");
    }

    #[test]
    fn host_formatting_quarantines_old_native_commit_without_losing_history() {
        let context = egui::Context::default();
        let mut editor = Editor::new("word");
        editor
            .set_selection(Selection {
                anchor: ByteOffset(0),
                head: ByteOffset(4),
                ..Selection::default()
            })
            .unwrap();
        let mut state = NativeViewState::default();
        state.request_focus();
        let _ = route(&context, &mut editor, &mut state, vec![], true);
        assert!(
            state
                .execute_command(&mut editor, EditCommand::ToggleStrong, &context)
                .unwrap()
        );
        let _ = route(
            &context,
            &mut editor,
            &mut state,
            vec![Event::Ime(egui::ImeEvent::Commit("old".into()))],
            true,
        );
        assert_eq!(editor.snapshot().text(), "**word**");
        editor.undo().unwrap();
        assert_eq!(editor.snapshot().text(), "word");
        assert!(!editor.can_undo());
    }

    #[test]
    fn native_double_and_triple_click_select_word_and_full_crlf_line() {
        let context = egui::Context::default();
        let mut editor = Editor::new("alpha   beta\r\nnext");
        let mut state = NativeViewState::default();
        state.request_focus();
        let _ = route(&context, &mut editor, &mut state, vec![], true);
        let local = state
            .last_surface
            .as_ref()
            .unwrap()
            .geometry
            .caret(ByteOffset(10), glyphfolio_editor_core::Affinity::After);
        let point = egui::pos2(local.x + 16.0, local.y + 16.0 + local.height * 0.5);
        for (index, time) in [1.0, 1.15, 1.3].into_iter().enumerate() {
            let mut output = context.run_ui(
                egui::RawInput {
                    time: Some(time),
                    focused: true,
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(640.0, 480.0))),
                    events: vec![
                        Event::PointerMoved(point),
                        Event::PointerButton {
                            pos: point,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers: Modifiers::NONE,
                        },
                        Event::PointerButton {
                            pos: point,
                            button: egui::PointerButton::Primary,
                            pressed: false,
                            modifiers: Modifiers::NONE,
                        },
                    ],
                    ..Default::default()
                },
                |ui| {
                    ui.add(EditorWidget::new(&mut editor, &mut state).id(Id::unique("route")));
                },
            );
            output.textures_delta.clear();
            if index == 1 {
                assert_eq!(editor.selection().range(), ByteOffset(8)..ByteOffset(12));
            }
            if index == 2 {
                assert_eq!(editor.selection().range(), ByteOffset(0)..ByteOffset(14));
            }
        }
        assert_eq!(editor.snapshot().text(), "alpha   beta\r\nnext");
        assert!(!editor.can_undo());
    }
    #[test]
    fn rich_source_toggle_preserves_bytes_selection_and_history() {
        let context = egui::Context::default();
        let original = "# Heading\n\n**bold** and &amp;\n\nlast";
        let mut editor = Editor::new(original);
        editor
            .set_selection(Selection::caret(ByteOffset(original.len())))
            .unwrap();
        let mut state = NativeViewState::default();
        state.enable_experimental_rich(true);
        state.request_focus();
        state.request_presentation(PresentationMode::Rich);
        let _ = route(&context, &mut editor, &mut state, vec![], true);
        let selected = editor.selection();
        assert_eq!(state.rendered_presentation(), PresentationMode::Rich);
        assert!(
            !state
                .last_surface
                .as_ref()
                .unwrap()
                .geometry
                .document
                .text
                .contains("**bold**")
        );
        let _ = route(
            &context,
            &mut editor,
            &mut state,
            vec![Event::Text("!".into())],
            true,
        );
        let edited = editor.snapshot().text();
        state.request_presentation(PresentationMode::Source);
        let _ = route(&context, &mut editor, &mut state, vec![], true);
        assert_eq!(
            state.last_surface.as_ref().unwrap().geometry.document.text,
            edited
        );
        assert!(editor.undo().unwrap().is_some());
        assert_eq!(editor.snapshot().text(), original);
        assert_eq!(editor.selection(), selected);
        assert!(!editor.can_undo());
    }

    #[test]
    fn rich_accessible_collapsed_caret_does_not_select_hidden_delimiters() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let original = "**bold**\n\nlast";
        let mut editor = Editor::new(original);
        editor
            .set_selection(Selection::caret(ByteOffset(original.len())))
            .unwrap();
        let mut state = NativeViewState::default();
        state.enable_experimental_rich(true);
        state.request_presentation(PresentationMode::Rich);
        state.request_focus();
        let _ = route(&context, &mut editor, &mut state, vec![], true);
        let pos = state.accessibility.position(ByteOffset(0)).unwrap();
        let request = accesskit::ActionRequest {
            action: accesskit::Action::SetTextSelection,
            target_tree: accesskit::TreeId::ROOT,
            target_node: Id::unique("route").accesskit_id(),
            data: Some(accesskit::ActionData::SetTextSelection(
                accesskit::TextSelection {
                    anchor: pos,
                    focus: pos,
                },
            )),
        };
        let _ = route(
            &context,
            &mut editor,
            &mut state,
            vec![Event::AccessKitActionRequest(request)],
            true,
        );
        assert!(editor.selection().is_collapsed());
        assert_eq!(editor.selection().head, ByteOffset(2));
        let _ = route(
            &context,
            &mut editor,
            &mut state,
            vec![Event::Text("X".into())],
            true,
        );
        assert_eq!(editor.snapshot().text(), "**Xbold**\n\nlast");
        editor.undo().unwrap();
        assert_eq!(editor.snapshot().text(), original);
    }

    #[test]
    fn exact_fallback_paint_culls_offscreen_glyphs_and_indexes_styles() {
        let context = egui::Context::default();
        let text = "word\n".repeat(2000);
        let document = ShapingEngine::new().shape(
            &text,
            LayoutOptions {
                width: 200.0,
                ..Default::default()
            },
        );
        let spans: Vec<_> = (0..2000)
            .map(|line| presentation::PaintSpan {
                display_range: line * 5..line * 5 + 4,
                kind: glyphfolio_editor_markdown::StyleKind::Syntax,
            })
            .collect();
        let mut state = NativeViewState::default();
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(240.0, 100.0))),
                ..Default::default()
            },
            |ui| {
                let painter = ui.painter_at(Rect::from_min_size(Pos2::ZERO, vec2(200.0, 80.0)));
                paint_shaped(
                    &context,
                    &painter,
                    &mut state,
                    &document,
                    Pos2::ZERO,
                    Color32::WHITE,
                    &spans,
                );
            },
        );
        output.textures_delta.clear();
        assert!(state.paint_metrics.considered_glyphs > 4000);
        assert!(state.paint_metrics.painted_glyphs < 100);
        assert!(state.paint_metrics.maximum_style_candidates_per_glyph <= 1);
        assert!(state.glyph_textures.len() < 20);
    }

    #[test]
    fn geometry_hits_legal_graphemes_and_roundtrips_carets() {
        let text = "office e\u{301} 👩‍👩‍👧‍👦\nsecond line";
        let geometry = geometry_for(text);
        let legal: Vec<_> = text
            .grapheme_indices(true)
            .map(|(byte, _)| byte)
            .chain([text.len()])
            .collect();
        assert!(geometry.selection_rects(5..5).is_empty());
        for row in &geometry.document.rows {
            for caret in &row.carets {
                assert!(legal.contains(&caret.byte.0));
                let hit = geometry.hit_test_caret(egui::pos2(caret.x, row.bounds.center().y));
                assert!(legal.contains(&hit.byte.0));
                assert!(
                    (geometry
                        .caret_with_affinity(hit.byte.0, hit.affinity)
                        .left()
                        - caret.x)
                        .abs()
                        < 0.1
                );
            }
        }
    }
    #[test]
    fn accessible_selection_replacement_uses_transactions() {
        let text = "a e\u{301}\nlast";
        let geometry = geometry_for(text);
        let id = Id::unique("test");
        let mut editor = Editor::new(text);
        let mut state = NativeViewState::default();
        state.accessibility.build(
            id,
            Rect::from_min_size(Pos2::ZERO, vec2(300.0, 300.0)),
            Pos2::ZERO,
            editor.snapshot().document_id(),
            &geometry.document,
            editor.selection(),
        );
        let request = accesskit::ActionRequest {
            action: accesskit::Action::SetTextSelection,
            target_tree: accesskit::TreeId::ROOT,
            target_node: id.accesskit_id(),
            data: Some(accesskit::ActionData::SetTextSelection(
                accesskit::TextSelection {
                    anchor: state.accessibility.position(ByteOffset(2)).unwrap(),
                    focus: state.accessibility.position(ByteOffset(5)).unwrap(),
                },
            )),
        };
        accessibility_action(&mut editor, &mut state, &request, id, &geometry, text).unwrap();
        let replace = accesskit::ActionRequest {
            action: accesskit::Action::ReplaceSelectedText,
            target_tree: accesskit::TreeId::ROOT,
            target_node: id.accesskit_id(),
            data: Some(accesskit::ActionData::Value("é".into())),
        };
        accessibility_action(&mut editor, &mut state, &replace, id, &geometry, text).unwrap();
        assert_eq!(editor.snapshot().text(), "a é\nlast");
        editor.undo().unwrap();
        assert_eq!(editor.snapshot().text(), text);
        let nodes = state.accessibility.build(
            id,
            Rect::from_min_size(Pos2::ZERO, vec2(300.0, 300.0)),
            Pos2::ZERO,
            editor.snapshot().document_id(),
            &geometry.document,
            Selection::caret(ByteOffset(0)),
        );
        assert_eq!(nodes[0].1.role(), accesskit::Role::MultilineTextInput);
        for (_, run) in nodes
            .iter()
            .filter(|(_, node)| node.role() == accesskit::Role::TextRun)
        {
            assert_eq!(run.role(), accesskit::Role::TextRun);
            assert_eq!(
                run.character_lengths()
                    .iter()
                    .map(|length| usize::from(*length))
                    .sum::<usize>(),
                run.value().unwrap_or_default().len()
            );
        }
    }
}
