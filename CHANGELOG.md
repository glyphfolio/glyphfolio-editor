# Changelog

## 0.1.0 preview.1 — 2026-10-28

Initial source preview of an original native Rust rich Markdown editor, with egui as its first adapter.

- One canonical Markdown document, revision-safe transactions, shared selection and undo/redo.
- Source and Rich presentation; folded formatting stays stable as the caret moves.
- CommonMark parsing plus opt-in GFM/wiki links, tasks and strikethrough.
- Editable native tables with wrapping, alignment, cell navigation and row/column commands.
- Formatting-aware typing, paragraph splitting/joining and source-preserving editing.
- Configurable toolbar, features, light/dark themes and standalone/embedding examples.
- Host-owned assets, bounded HTML paste conversion and native input/accessibility infrastructure.
- Block virtualization, property/regression tests and explicit native evidence.

This is an experimental pre-1.0 API, not a stable release or a qualified drop-in replacement for CodeMirror in GlyphFolio. See [preview status](docs/PREVIEW_RELEASE.md) for limitations and verification scope.
