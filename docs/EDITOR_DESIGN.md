# GlyphFolio appearance and configurable native editor

The editor now follows GlyphFolio's existing writing pane, while keeping the original Rust document, transactions, history, commands, Source/Rich projections, native input and accessibility transport. Neither GlyphFolio application checkout was modified.

## Reference and implementation

The visual reference was the current Tauri application's packaged light/dark editor screenshots under `glyphfolio-tauri-migration/docs/validation/m12/regression-a55a3ac6-576f-4130-97bf-512f781af4de/editor/`, checked against its current `src/renderer/style.css`, `theme.ts` and settings defaults. Its stylesheet matches the original GlyphFolio application. Only visual tokens and layout were used as references; no existing editor engine/controller was copied or adapted.

The native pane uses a centered 760-point comfortable column with 32-point outer gutters, 16-point body text and 1.6 line spacing. It matches the white / `#26282c` surfaces, primary and muted text, accessible blue accents, compact 26-point icon controls, thin toolbar rules, blue Rich/Source segmented control, heading hierarchy and code/quote treatment. Source uses native monospace shaping. The standalone begins at the editor toolbar; it has no application header, note-title label or decorative metadata. No font or raster icon asset is bundled.

An embedding application owns any document title, notebook/navigation metadata and save service. Those application elements are omitted from this repository's standalone editor surface. Settings remain available in the toolbar's Editor options menu. The library provides the editor itself through `EditorPanel`, or `EditorWidget` and `EditorToolbar` separately. The original Source diagnostic fixture remains available through `--native-test-ui`; input probe modes use it automatically and it ignores user configuration files.

## Configuration and retained features

[editor.toml](../editor.toml) is the standalone profile. [Configuration documentation](CONFIGURATION.md) describes all fields and the public Rust API. [Minimal](../config/minimal.toml) and [full](../config/full.toml) examples show the two ends of the supplied controls.

- Toolbar groups choose controls and their order; the presentation selector stays anchored on the right.
- Feature flags independently disable supplied commands, shortcuts and clipboard/link interactions. Hiding a toolbar control leaves its feature available.
- Themes, font size, line spacing, column width, gutters, optional frame/status/word count and diagnostics can be configured.
- Changing settings keeps the same document, selection and undo history. Active composition retains the existing deferred mode-change contract.
- Bold/italic/code, headings, lists, quotes, fences, indent/outdent, selection/navigation, Source/Rich, undo/redo and clipboard integration remain implemented. Developer/native-input controls are available in the full profile and original test fixture.
- Save buttons emit a host request. The standalone uses the existing atomic snapshot save service, with an explicit path or its Save Markdown dialog. The editor library never chooses a file or writes one automatically.

The settings window provides live window controls and reloads the explicit TOML file; persistent defaults come from that file. Configuration is part of the egui adapter. The core, Markdown and layout crates remain independent of egui and GlyphFolio application services.

## Validation

The local workspace has **189 passing tests**, including the existing editing/fidelity suite, eight configuration contract tests, six configured event-route tests, toolbar pointer/action tests, panel/viewport tests and typography reconfiguration tests. They verify source/history preservation, disabled shortcuts and transport, settings changes during composition, invalid-setting rejection, host intents and retained writing-surface height.

Native smoke captures cover light/dark, 480- and 320-point widths, the minimal Source profile and full developer profile. Screenshots were inspected against the reference. This visual increment does not complete the earlier open screen-reader/platform qualification gates or change the declared limitations of experimental Rich editing.

- [Light editor](evidence/editor-design/light.png)
- [Dark editor](evidence/editor-design/dark.png)
- [480-point window](evidence/editor-design/narrow.png)
- [320-point window](evidence/editor-design/compact.png)
- [Minimal configuration](evidence/editor-design/minimal.png)
- [Full configuration with diagnostics](evidence/editor-design/full.png)

Reproduce:

```sh
cargo run --locked -p glyphfolio-editor-demo -- --theme light
cargo run --locked -p glyphfolio-editor-demo -- --theme dark
cargo run --locked -p glyphfolio-editor-demo -- --window-width 480
cargo run --locked -p glyphfolio-editor-demo -- --config config/minimal.toml
cargo run --locked -p glyphfolio-editor-demo -- --config config/full.toml
cargo test --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
```
