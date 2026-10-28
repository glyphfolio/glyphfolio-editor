# Configuring the native editor

The editor has a typed configuration and a TOML file format. A host can choose
its toolbar, command availability, initial presentation, theme, typography,
document column, status information and developer controls. Configuration is
part of the native adapter; the document/transaction engine remains independent
of egui, application settings and file-system discovery.

The standalone example uses the workspace's explicit `editor.toml` path. It
does not search the current directory for settings. To choose a different file:

```sh
cargo run --locked -p glyphfolio-editor-demo -- --config config/minimal.toml
cargo run --locked -p glyphfolio-editor-demo -- --config config/full.toml
```

`editor.toml` follows the GlyphFolio editor's light/dark appearance. `minimal.toml`
shows a compact Source toolbar. `full.toml` retains every implemented tool and
also exposes performance/native-input controls.

## Toolbar and features

Toolbar groups are ordered arrays. Moving a group or an item changes its
placement among the command groups. The presentation switch is anchored to the right in the GlyphFolio toolbar. Removing an item hides its button while preserving its keyboard/API
behavior. The `features` table separately disables the supplied command or
interaction, including its editor shortcut.

```toml
schema_version = 1

[toolbar]
visible = true
groups = [
  ["heading"],
  ["bold", "italic", "inline_code"],
  ["undo", "redo"],
  ["presentation"],
]

[features]
code_fence = false
```

Supported toolbar items: `undo`, `redo`, `heading`, `bold`, `italic`,
`strikethrough`, `link`, `inline_code`, `bullet_list`, `ordered_list`, `task_list`,
`task_checked`, `table`, `paste`, `blockquote`, `code_fence`,
`indent`, `outdent`, `presentation`, and `save`. Save remains a host operation;
the standalone example uses `--save-path` when supplied and otherwise opens its host-owned Save Markdown dialog. The library never writes a file in response to a toolbar button.

Feature flags: `bold`, `italic`, `inline_code`, `headings`, `lists`, `blockquote`,
`code_fence`, `indentation`, `links`, `strikethrough`, `task_lists`, `tables`,
`formatted_paste`, `images`, `attachments`, `history`, `clipboard`, `source`, and `rich`.
`images` controls resolved image presentation; `attachments` controls imports. Hiding
or disabling these controls does not remove existing references or source text.
Unspecified flags remain enabled. Disabling a feature never deletes Markdown
already present in a document, removes history entries, or changes saved bytes.
Base text editing and navigation remain available. The unrestricted headless
core API remains under the embedding host's control; these flags are editor UI
policy, not a security boundary or Markdown content sanitizer.

## Presentation and appearance

```toml
[editor]
default_presentation = "rich"
allow_experimental_rich = true

[appearance]
theme = "system" # system, light, dark
frame = false

[document]
font_size = 16.0
line_height = 1.6
max_width = 760.0
padding = 32.0
inner_padding = 0.0

[status]
visible = true
word_count = true

[diagnostics]
visible = false
native_input_controls = false
```

Dimensions use egui points; `line_height` is a font-size multiplier.
`padding` controls the outer horizontal gutters, and `inner_padding` controls
the editing-surface inset. The host still supplies the available window width.

`EditorConfig::default()` begins in Source with experimental Rich disabled.
`EditorConfig::glyphfolio()` opts into Rich, while keeping all editing features
available. Rich's existing native qualification limits and large-paragraph
Source safeguard still apply. Disabling the Source selector does not suppress
an internal Source safeguard needed for correct composition or large documents.

## Embedding and validation

```rust,no_run
use glyphfolio_editor_egui::{NativeViewState, config::EditorConfig};

let config = EditorConfig::load("/explicit/host/path/editor.toml")?;
config.validate()?;
let mut view = NativeViewState::default();
view.configure(&config)?;
let exported_toml = config.to_toml()?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Hosts can also call `EditorConfig::from_toml(&text)` or construct the typed
configuration directly. Loading reads only the supplied path. Exporting returns
a string; a host owns saving and reload timing. Configuration loading does not
create files, initialize a new document, clear selection/history or touch the
current GlyphFolio application.

Unknown keys, unknown tool/theme/presentation names, unsupported schema
versions, duplicate toolbar items, invalid geometry and conflicting initial
presentation settings are errors. Sparse files inherit documented defaults.
Load/validate a replacement before applying it so an invalid file leaves the
active editor untouched.

## Drop-in egui panel

`EditorPanel` composes the toolbar, centered writing column, scrollable surface
and optional status line. It uses the same `Editor` and `NativeViewState`;
changing configuration never creates a replacement document.

```rust
use glyphfolio_editor_egui::{EditorPanel, config::EditorConfig};

let config = EditorConfig::load("editor.toml")?;
let output = EditorPanel::new(&mut editor, &mut view, &config)
    .max_height(600.0)
    .show(ui)?;
if output.save_requested {
    // Pass editor.snapshot() to your host's save service.
}
if output.settings_requested {
    // Show your host's editor configuration controls.
}
if output.diagnostics_requested {
    // Toggle your host's diagnostics view.
}
```

Use `.with_gutters(false)` when the host already centers the editor, and
`.show_status(false)` when it provides its own footer. Give multiple panels
explicit `.id(...)` values. `EditorWidget` and `EditorToolbar` remain available
separately for hosts that supply their own layout. The standalone app uses this
same panel beneath its document header. Its settings window applies changes to
the current window; edit the TOML file to choose persistent defaults, then use
**Reload configuration file**.

The diagnostic fixture is preserved separately: `--native-test-ui` displays the
original Source surface and directly visible native-input test controls.
Existing native IME probe modes select that fixture automatically.

Presentation feature flags control user-selectable views. The editor retains its
existing Source safety fallbacks for composition and pathological Rich blocks;
configuration cannot disable source-fidelity safeguards. The original native
test fixture ignores user configuration files so visual customization cannot
break its synthetic platform-input setup.
