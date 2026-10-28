//! Standalone original Source surface and opt-in native feasibility probes.
mod attachments;
mod native_probe;
mod workspace_ui;
use eframe::egui;
use glyphfolio_editor_core::{
    ByteOffset, EditCommand, Editor, HeadingLevel, PresentationMode, Selection,
};
use glyphfolio_editor_egui::{
    EditorWidget, NativeViewState, clipboard::SystemClipboard, config::EditorConfig,
};
use native_probe::{NativeKeyProbe, ProbeMode, ProbeProgress};
use std::{path::PathBuf, time::Instant};

const SAMPLE: &str = "# glyphfolio-editor\n\nOriginal Rust engine. Native editing feasibility surface.\n\n## One document, two presentations\n\nSelect **bold**, *emphasis*, `inline code`, or [a link](https://example.com). Source and experimental Rich share the same document and undo history.\n\n> A native Markdown editor built from scratch.\n\n- Edit text and select across blocks\n- Format, copy/cut/paste, undo, and redo\n\n1. Source is the default presentation\n2. Rich is experimental while native qualification is incomplete\n\n```rust\nlet editor = Editor::new(\"Hello\");\n```\n\n## International text\n\nLatin: office affinity — café — é\nJapanese: 日本語を入力\nChinese: 中文输入\nIndic: नमस्ते\nEmoji: 👩‍👩‍👧‍👦 👍🏽\nMixed direction: English العربية עברית 123\n\nNative IME and VoiceOver require their own platform evidence.\n";
const PROBE_SOURCE: &str = "left TARGET right";
const WELCOME: &str = "# A space for your ideas\n\nCapture a thought. Shape it into something worth keeping.\n\n## Make it yours\n\nKeep your writing simple, or add **a little emphasis**, *a softer note*, and `a line of code`. Switch between Rich and Source whenever you like.\n\n> Good tools leave room for your ideas.\n\n- Gather the details that matter\n- Give your thoughts a little structure\n- Keep everything in plain Markdown\n\n## From a thought to a plan\n\n1. Start with a small idea\n2. Find the shape of the story\n3. Make something worth sharing\n\n```rust\nlet idea = \"Something worth making\";\n```\n\nYour document stays yours.\n";

struct Demo {
    editor: Editor,
    view: NativeViewState,
    frame: usize,
    smoke: bool,
    smoke_frames: usize,
    ui_times: Vec<u64>,
    screenshot: Option<PathBuf>,
    screenshot_requested: bool,
    screenshot_saved: bool,
    screenshot_result: Option<std::sync::mpsc::Receiver<Result<PathBuf, String>>>,
    started: Instant,
    probe: Option<NativeKeyProbe>,
    probe_mode: Option<ProbeMode>,
    failed: bool,
    closed: bool,
    observe: bool,
    last_focus: Option<bool>,
    viewport_probe: bool,
    viewport_scaled: bool,
    viewport_scrolled: bool,
    mode_close_deferred: bool,
    save_path: Option<PathBuf>,
    save_message: Option<String>,
    config: EditorConfig,
    config_path: PathBuf,
    native_test_ui: bool,
    diagnostics_visible: bool,
    settings_open: bool,
    save_dialog_open: bool,
    save_location: String,
    counted_snapshot: Option<(
        glyphfolio_editor_core::DocumentId,
        glyphfolio_editor_core::Revision,
    )>,
    word_count: usize,
}
impl Demo {
    fn finish_probe(&mut self, context: &egui::Context) {
        let mode = self.probe_mode.unwrap();
        let text = self.editor.snapshot().text();
        let native_composition =
            self.view.trace.preedit_events > 0 && self.view.trace.commit_events > 0;
        let expected = match mode {
            ProbeMode::Latin => text == "left probe right",
            ProbeMode::JapaneseRomaji => {
                text == "left 日本 right"
                    && native_composition
                    && self.editor.snapshot().revision().0 == 1
            }
            ProbeMode::JapaneseCancel => {
                text == PROBE_SOURCE
                    && self.editor.snapshot().revision().0 == 0
                    && self.view.trace.preedit_events > 0
                    && self.view.trace.commit_events == 0
                    && !self.view.is_composing()
            }
            ProbeMode::ChinesePinyin => {
                text == "left 中文 right"
                    && native_composition
                    && self.editor.snapshot().revision().0 == 1
            }
            ProbeMode::JapaneseFocusLoss => {
                self.view.trace.preedit_events > 0
                    && !self.view.is_composing()
                    && ((text == PROBE_SOURCE && self.editor.snapshot().revision().0 == 0)
                        || (text == "left にほん right"
                            && native_composition
                            && self.editor.snapshot().revision().0 == 1))
            }
        };
        let stable_while_preediting = !self.failed;
        let mut undo_restored = false;
        let requires_undo = matches!(mode, ProbeMode::JapaneseRomaji | ProbeMode::ChinesePinyin)
            || (mode == ProbeMode::JapaneseFocusLoss && text != PROBE_SOURCE);
        if requires_undo && expected {
            undo_restored = self.editor.undo().is_ok_and(|change| change.is_some())
                && self.editor.snapshot().text() == PROBE_SOURCE;
            let _ = self.editor.redo();
        }
        let passed = expected
            && stable_while_preediting
            && (!requires_undo || undo_restored)
            && (!self.viewport_probe
                || (self.viewport_scaled && self.viewport_scrolled && self.mode_close_deferred));
        if self.viewport_probe {
            println!(
                "NATIVE_VIEWPORT scaled_while_composing={} scrolled_while_composing={} mode_and_close_deferred={}",
                self.viewport_scaled, self.viewport_scrolled, self.mode_close_deferred
            );
        }
        println!(
            "NATIVE_INPUT mode={mode:?} result={} preedit={} commit={} text_events={} rejected={} provisional_source_unchanged={} undo_restored={} source={text:?}",
            if passed { "PASS" } else { "FAIL" },
            self.view.trace.preedit_events,
            self.view.trace.commit_events,
            self.view.trace.text_events,
            self.view.trace.rejected_events,
            stable_while_preediting,
            undo_restored
        );
        self.failed |= !passed;
        self.probe = None;
        // A failed cancellation must not leave the probe hanging on its close barrier.
        if self.view.is_composing() {
            self.view.cancel_composition();
        }
        if self.screenshot.is_none() || self.screenshot_saved {
            self.closed = true;
            context.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
impl Demo {
    fn show_native_test_ui(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("glyphfolio-editor");
            ui.label("Original engine · Native Rust · COSMIC Text shaping · No WebView");
            ui.horizontal(|ui| {
                if ui.button("Source").clicked() {
                    self.view.request_presentation(PresentationMode::Source);
                }
                if ui.button("Rich (experimental)").clicked() {
                    self.view.enable_experimental_rich(true);
                    self.view.request_presentation(PresentationMode::Rich);
                }
                if ui
                    .add_enabled(
                        self.editor.can_undo() && !self.view.is_composing(),
                        egui::Button::new("Undo"),
                    )
                    .clicked()
                {
                    let _ = self.editor.undo();
                }
                if ui
                    .add_enabled(
                        self.editor.can_redo() && !self.view.is_composing(),
                        egui::Button::new("Redo"),
                    )
                    .clicked()
                {
                    let _ = self.editor.redo();
                }
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
            if self.view.presentation() == PresentationMode::Rich {
                ui.colored_label(egui::Color32::YELLOW, "Experimental Rich: native screen-reader qualification remains open; composition temporarily reveals Source.");
            }
            if self.view.render_metrics().is_some_and(|metrics| metrics.large_rich_source_fallback) {
                ui.colored_label(egui::Color32::YELLOW, "Large styled paragraph shown as Source to keep editing responsive.");
            }
            ui.add_enabled_ui(!self.view.is_composing(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (label, command) in [("Bold", EditCommand::ToggleStrong), ("Italic", EditCommand::ToggleEmphasis), ("Code", EditCommand::ToggleInlineCode),
                        ("Bullet list", EditCommand::ToggleUnorderedList), ("Numbered list", EditCommand::ToggleOrderedList), ("Quote", EditCommand::ToggleBlockQuote),
                        ("Code fence", EditCommand::ToggleCodeFence), ("Indent", EditCommand::Indent), ("Outdent", EditCommand::Outdent)] {
                        if ui.button(label).clicked() && let Err(error) = self.view.execute_command(&mut self.editor, command, context) { self.view.last_error = Some(error); }
                    }
                    egui::ComboBox::from_id_salt("heading").selected_text("Heading").show_ui(ui, |ui| {
                        for (label, heading) in [("Body", None), ("H1", Some(HeadingLevel::One)), ("H2", Some(HeadingLevel::Two)), ("H3", Some(HeadingLevel::Three)),
                            ("H4", Some(HeadingLevel::Four)), ("H5", Some(HeadingLevel::Five)), ("H6", Some(HeadingLevel::Six))] {
                            if ui.button(label).clicked() && let Err(error) = self.view.execute_command(&mut self.editor, EditCommand::SetHeading(heading), context) { self.view.last_error = Some(error); }
                        }
                    });
                    if let Some(path) = self.save_path.as_ref() && ui.button("Save source").clicked() {
                        let result = glyphfolio_editor_host_example::save_snapshot_atomic(self.editor.snapshot(), path)
                            .map_err(|error| error.to_string()).and_then(|receipt| receipt.acknowledge(&mut self.editor).map(|_| format!("Saved {}", receipt.path().display())).map_err(|error| error.to_string()));
                        match result { Ok(message) => self.save_message = Some(message), Err(error) => self.view.last_error = Some(error) }
                    }
                });
            });
            if let Some(message) = &self.save_message { ui.label(message); }
            ui.label(format!(
                "revision={} · {} bytes · dirty={} · composing={} · rows={}",
                self.editor.snapshot().revision().0,
                self.editor.snapshot().byte_len(),
                self.editor.is_dirty(),
                self.view.is_composing(),
                self.view.measured_rows
            ));
            if let Some(diagnostics) = self.view.shaping_diagnostics() {
                ui.label(format!(
                    "fonts={} · missing={} · raster failures={} · color glyphs={}",
                    diagnostics.font_faces,
                    diagnostics.missing_glyphs,
                    diagnostics.raster_failures,
                    diagnostics.color_glyphs
                ));
            }
            if let Some(metrics) = self.view.render_metrics() {
                ui.label(format!("blocks={} · shaped={} · cached={} ({:.1} MiB accounted) · layout={:.2} ms · full AX={}", metrics.total_blocks, metrics.shaped_blocks, metrics.cache_entries, metrics.cache_bytes as f64 / 1_048_576.0, metrics.layout_microseconds as f64 / 1000.0, metrics.accessibility_full_layout));
            }
            let source_scroll = egui::ScrollArea::vertical()
                .id_salt("source-scroll")
                .max_height(if self.viewport_probe {
                    100.0
                } else {
                    f32::INFINITY
                })
                .show(ui, |ui| {
                    ui.add(EditorWidget::new(&mut self.editor, &mut self.view));
                });
            if self.viewport_probe
                && self.view.is_composing()
                && source_scroll.state.clamped_offset().y > 0.0
            {
                self.viewport_scrolled = true;
            }
            if let Some(error) = &self.view.last_error {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
        });
    }
}
impl eframe::App for Demo {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if (self.smoke || self.probe_mode.is_some()) && (!self.closed || self.failed) {
            eprintln!("NATIVE_PROBE_INCOMPLETE");
            std::process::exit(2);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ui_started = Instant::now();
        let context = ui.ctx().clone();
        if self.viewport_probe && self.view.is_composing() && !self.viewport_scaled {
            context.set_zoom_factor(1.25);
            self.view.request_presentation(PresentationMode::Rich);
            self.mode_close_deferred =
                self.view.presentation() == PresentationMode::Source && !self.view.can_close();
            self.viewport_scaled = true;
        }
        if self.observe {
            let focused = context.input(|input| input.focused);
            if self.last_focus != Some(focused) {
                self.last_focus = Some(focused);
                println!(
                    "NATIVE_OBSERVER_FOCUS window={focused} widget={}",
                    context.memory(|memory| memory.focused().is_some())
                );
            }
            context.input(|input| {
                for event in &input.events {
                    if matches!(
                        event,
                        egui::Event::Ime(_)
                            | egui::Event::Key { .. }
                            | egui::Event::WindowFocused(_)
                            | egui::Event::PointerButton { .. }
                    ) {
                        println!("NATIVE_OBSERVER_EVENT {event:?}");
                    }
                }
            });
        }
        if context.input(|input| input.viewport().close_requested()) && !self.view.can_close() {
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        if let Some(receiver) = &self.screenshot_result
            && let Ok(result) = receiver.try_recv()
        {
            match result {
                Ok(path) => {
                    self.screenshot_saved = true;
                    println!("SCREENSHOT_SAVED {}", path.display());
                }
                Err(error) => {
                    self.failed = true;
                    eprintln!("SCREENSHOT_ERROR {error}");
                }
            }
        }
        if self.native_test_ui {
            self.show_native_test_ui(ui, &context);
        } else {
            self.show_workspace_ui(ui, &context);
        }
        self.ui_times.push(ui_started.elapsed().as_micros() as u64);
        self.frame += 1;
        if self.frame == 1 {
            // This standalone test app explicitly owns its window. Embedding
            // hosts decide when their window should acquire native focus.
            context.send_viewport_cmd(egui::ViewportCommand::Focus);
            println!(
                "NATIVE_READY pid={} focused={} candidate={:?}",
                std::process::id(),
                context.input(|input| input.focused),
                self.view.candidate_rect
            );
        }
        if self.frame >= 4 && self.screenshot.is_some() && !self.screenshot_requested {
            self.screenshot_requested = true;
            let path = self.screenshot.clone().unwrap();
            let (sender, receiver) = std::sync::mpsc::channel();
            self.screenshot_result = Some(receiver);
            let repaint = context.clone();
            context.request_screenshot(move |screenshot| {
                let result = image::save_buffer(
                    &path,
                    screenshot.as_raw(),
                    screenshot.width() as u32,
                    screenshot.height() as u32,
                    image::ColorType::Rgba8,
                )
                .map(|()| path)
                .map_err(|error| error.to_string());
                let _ = sender.send(result);
                repaint.request_repaint();
            });
        }
        if self.observe && !self.closed {
            if self.view.is_composing() && self.editor.snapshot().text() != PROBE_SOURCE {
                self.failed = true;
            }
            context.request_repaint_after(std::time::Duration::from_millis(30));
            if self.started.elapsed().as_secs() >= 12 {
                self.finish_probe(&context);
            }
        } else if self.probe.is_some() {
            if self.view.is_composing() && self.editor.snapshot().text() != PROBE_SOURCE {
                self.failed = true;
            }
            context.request_repaint_after(std::time::Duration::from_millis(30));
            let progress = self.probe.as_mut().unwrap().tick();
            match progress {
                Ok(ProbeProgress::Complete) => self.finish_probe(&context),
                Ok(ProbeProgress::Posted { index, total, key }) => {
                    println!("NATIVE_KEY posted={index}/{total} key={key}")
                }
                Ok(_) => {}
                Err(error) => {
                    self.failed = true;
                    eprintln!("NATIVE_INPUT_ERROR {error}");
                    self.probe = None;
                    self.view.cancel_composition();
                    context.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        } else if self.smoke && !self.closed {
            context.request_repaint();
            if self.frame >= self.smoke_frames
                && (self.screenshot.is_none() || self.screenshot_saved)
            {
                println!(
                    "NATIVE_SMOKE frames={} revision={} rows={} screenshot={} os_ime=NOT_RUN screen_reader=NOT_RUN",
                    self.frame,
                    self.editor.snapshot().revision().0,
                    self.view.measured_rows,
                    self.screenshot_saved
                );
                if let Some(metrics) = self.view.render_metrics() {
                    println!(
                        "RENDER_METRICS mode={:?} {metrics:?}",
                        self.view.rendered_presentation()
                    );
                }
                let mut warm = self.ui_times.iter().skip(3).copied().collect::<Vec<_>>();
                warm.sort_unstable();
                println!(
                    "UI_CALLBACK_TIMES cold_us={} warm_samples={} warm_p95_us={} warm_max_us={} elapsed_ms={} gpu_time=NOT_MEASURED paint={:?}",
                    self.ui_times[0],
                    warm.len(),
                    warm.get(warm.len().saturating_sub(1) * 95 / 100)
                        .copied()
                        .unwrap_or(0),
                    warm.last().copied().unwrap_or(0),
                    self.started.elapsed().as_millis(),
                    self.view.paint_metrics
                );
                self.closed = true;
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        if (self.smoke || self.probe_mode.is_some())
            && self.started.elapsed().as_secs() > 25
            && !self.closed
        {
            self.failed = true;
            self.closed = true;
            self.view.cancel_composition();
            eprintln!("NATIVE_PROBE_TIMEOUT");
            context.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.closed && self.failed {
            std::process::exit(2);
        }
    }
}
fn main() -> eframe::Result {
    let mut smoke = false;
    let mut smoke_frames = 12;
    let mut screenshot = None;
    let mut font = None;
    let mut probe_mode = None;
    let mut observe = false;
    let mut viewport_probe = false;
    let mut rich = false;
    let mut source_path = None;
    let mut asset_root = None;
    let mut generated_lines = None;
    let mut save_path = None;
    let mut config_path = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../editor.toml"));
    let mut native_test_ui = false;
    let mut show_diagnostics = false;
    let mut requested_theme = None;
    let mut window_width = 1050.0_f32;
    let mut window_height = 850.0_f32;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--window-width" => {
                window_width = args
                    .next()
                    .and_then(|value| value.parse::<f32>().ok())
                    .filter(|value| (320.0..=4096.0).contains(value))
                    .expect("Expected width 320..4096")
            }
            "--window-height" => {
                window_height = args
                    .next()
                    .and_then(|value| value.parse::<f32>().ok())
                    .filter(|value| (320.0..=4096.0).contains(value))
                    .expect("Expected height 320..4096")
            }
            "--config" => {
                config_path = args
                    .next()
                    .map(PathBuf::from)
                    .expect("Expected explicit configuration file path")
            }
            "--native-test-ui" => native_test_ui = true,
            "--diagnostics" => show_diagnostics = true,
            "--theme" => {
                requested_theme = Some(match args.next().as_deref() {
                    Some("light") => glyphfolio_editor_egui::config::ThemeMode::Light,
                    Some("dark") => glyphfolio_editor_egui::config::ThemeMode::Dark,
                    Some("system") => glyphfolio_editor_egui::config::ThemeMode::System,
                    _ => panic!("Expected --theme light|dark|system"),
                })
            }
            "--smoke" => smoke = true,
            "--smoke-frames" => {
                smoke = true;
                smoke_frames = args
                    .next()
                    .and_then(|value| value.parse().ok())
                    .filter(|value| (12..=10000).contains(value))
                    .expect("Expected frame count 12..10000");
            }
            "--experimental-rich" => rich = true,
            "--asset-root" => asset_root = args.next().map(PathBuf::from),
            "--source-file" => source_path = args.next().map(PathBuf::from),
            "--save-path" => save_path = args.next().map(PathBuf::from),
            "--generate-lines" => {
                generated_lines = Some(
                    args.next()
                        .and_then(|value| value.parse::<usize>().ok())
                        .filter(|value| *value <= 1_000_000)
                        .expect("Expected line count up to 1000000"),
                )
            }
            "--screenshot" => screenshot = args.next().map(PathBuf::from),
            "--font" => font = args.next().map(PathBuf::from),
            "--native-ime-viewport" => viewport_probe = true,
            "--native-ime-observe" => {
                observe = true;
                probe_mode = Some(match args.next().as_deref() {
                    Some("latin") => ProbeMode::Latin,
                    Some("japanese") => ProbeMode::JapaneseRomaji,
                    Some("cancel") => ProbeMode::JapaneseCancel,
                    Some("chinese") => ProbeMode::ChinesePinyin,
                    Some("focus") => ProbeMode::JapaneseFocusLoss,
                    _ => {
                        eprintln!("Expected latin, japanese, chinese, or cancel");
                        std::process::exit(2);
                    }
                });
            }
            "--native-ime-probe" => {
                probe_mode = Some(match args.next().as_deref() {
                    Some("latin") => ProbeMode::Latin,
                    Some("japanese") => ProbeMode::JapaneseRomaji,
                    Some("cancel") => ProbeMode::JapaneseCancel,
                    Some("chinese") => ProbeMode::ChinesePinyin,
                    _ => {
                        eprintln!("Expected latin, japanese, chinese, or cancel");
                        std::process::exit(2);
                    }
                })
            }
            "--help" => {
                println!(
                    "glyphfolio-editor-demo [--window-width POINTS] [--window-height POINTS] [--config PATH.toml] [--theme light|dark|system] [--diagnostics] [--native-test-ui] [--smoke] [--smoke-frames N] [--experimental-rich] [--source-file PATH.md] [--generate-lines N] [--save-path PATH.md] [--screenshot PATH.png] [--font PATH] [--native-ime-probe latin|japanese|chinese|cancel] [--native-ime-observe latin|japanese|chinese|cancel|focus] [--native-ime-viewport]"
                );
                return Ok(());
            }
            _ => {
                eprintln!("Unknown argument: {arg}");
                std::process::exit(2);
            }
        }
    }
    let native_test_ui = native_test_ui || probe_mode.is_some();
    let mut config = if native_test_ui {
        EditorConfig::default()
    } else {
        match EditorConfig::load(&config_path) {
            Ok(config) => config,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(2);
            }
        }
    };
    if let Some(theme) = requested_theme {
        config.appearance.theme = theme;
    }
    if show_diagnostics {
        config.diagnostics.visible = true;
        config.diagnostics.native_input_controls = true;
    }
    if rich {
        config.editor.default_presentation = glyphfolio_editor_egui::config::Presentation::Rich;
        config.editor.allow_experimental_rich = true;
    }
    if let Err(error) = config.validate() {
        eprintln!("{error}");
        std::process::exit(2);
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([window_width, window_height])
            .with_title("GlyphFolio Editor"),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "glyphfolio-editor",
        options,
        Box::new(move |creation| {
            if native_test_ui {
                creation.egui_ctx.set_visuals(egui::Visuals::dark());
            } else {
                glyphfolio_editor_egui::chrome::apply_style(
                    &creation.egui_ctx,
                    config.appearance.theme,
                );
            }
            let loaded = if let Some(path) = &source_path {
                std::fs::read_to_string(path).expect("UTF-8 source file must be readable")
            } else if let Some(count) = generated_lines {
                (0..count)
                    .map(|line| format!("Line {line}: **Markdown** 日本 😀\n"))
                    .collect::<String>()
            } else {
                if native_test_ui {
                    SAMPLE.to_owned()
                } else {
                    WELCOME.to_owned()
                }
            };
            let mut editor = Editor::new(if probe_mode.is_some() {
                PROBE_SOURCE
            } else {
                &loaded
            });
            if probe_mode.is_some() {
                editor
                    .set_selection(Selection {
                        anchor: ByteOffset(5),
                        head: ByteOffset(11),
                        ..Selection::default()
                    })
                    .unwrap();
            }
            let mut view = NativeViewState::default();
            if !native_test_ui {
                let root = asset_root
                    .clone()
                    .or_else(|| {
                        source_path
                            .as_ref()
                            .and_then(|path| path.parent().map(PathBuf::from))
                    })
                    .or_else(|| {
                        save_path
                            .as_ref()
                            .and_then(|path| path.parent().map(PathBuf::from))
                    })
                    .unwrap_or_else(|| std::env::temp_dir().join("glyphfolio-editor-demo"));
                match attachments::LocalAttachments::new(root) {
                    Ok(service) => view.set_asset_service(Box::new(service)),
                    Err(error) => view.last_error = Some(error),
                }
            }
            if !native_test_ui {
                view.configure(&config)
                    .expect("Validated editor configuration");
                if source_path.is_none() && generated_lines.is_none() {
                    let _ = editor.set_selection(Selection::caret(ByteOffset(
                        WELCOME.find("Capture").unwrap_or(0),
                    )));
                }
            } else {
                view.enable_experimental_rich(rich);
                if rich {
                    view.request_presentation(PresentationMode::Rich);
                }
            }
            view.request_focus();
            match SystemClipboard::new() {
                Ok(service) => view.set_clipboard(Box::new(service)),
                Err(error) => view.last_error = Some(error),
            }
            if let Some(path) = font
                && let Err(error) = view.load_font_file(&path)
            {
                view.last_error = Some(error);
            }
            Ok(Box::new(Demo {
                editor,
                view,
                frame: 0,
                smoke,
                smoke_frames,
                ui_times: Vec::new(),
                screenshot,
                screenshot_requested: false,
                screenshot_saved: false,
                screenshot_result: None,
                started: Instant::now(),
                probe: if observe {
                    None
                } else {
                    probe_mode.map(NativeKeyProbe::new)
                },
                probe_mode,
                observe,
                last_focus: None,
                viewport_probe,
                viewport_scaled: false,
                viewport_scrolled: false,
                mode_close_deferred: false,
                save_path: save_path.clone(),
                save_message: None,
                diagnostics_visible: config.diagnostics.visible,
                save_location: save_path.as_ref().map_or_else(
                    || "glyphfolio-note.md".to_owned(),
                    |path| path.display().to_string(),
                ),
                config,
                config_path,
                native_test_ui,
                settings_open: false,
                save_dialog_open: false,
                counted_snapshot: None,
                word_count: 0,
                failed: false,
                closed: false,
            }))
        }),
    )
}
