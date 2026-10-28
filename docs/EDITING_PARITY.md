# Rich editing parity: first implementation increment

2026-10-28. Original implementation in this independent workspace. Neither GlyphFolio application was edited. This increment advances roadmap items 1–5; it does not establish complete CodeMirror replacement or application integration.

## Available now

| Area | Implemented behavior |
| --- | --- |
| Grammar | Explicit `MarkdownDialect::{CommonMark,Gfm,GfmWiki}`. Native Rich uses tables, task lists, strikethrough and simple `[[note]]` / `[[note|label]]` wiki links. CommonMark remains the low-level parser default and retains its official corpus checks. |
| Marks and tasks | Strikethrough toolbar/command/rendering; task-list creation/removal, completion toggle, clickable checkboxes and task continuation/empty-item exit. All changes use the original transaction/history engine. |
| Tables | Native shaped cells with wrapping, measured alignment, header styling and horizontal scrolling. Insert/clear cells, add/delete rows and columns, promote a deleted header, delete the final column/table, and navigate with Tab/Enter. Pipe escaping and `<br>` cell breaks preserve Markdown. Unsupported/ragged projections retain source fallback. |
| Links | Toolbar dialog inserts/edits/removes links and simple wiki links, with revision checks and undo. Modifier-click exposes activation to the host; `wiki:` destinations are host note-resolution requests. |
| Images/assets | Public `AssetService` owns resolution/import. Resolved images paint natively below their Markdown label, with editable Markdown available in Source mode. The reusable adapter imports dropped paths, native `PasteImage` events and explicit Paste-toolbar file/image clipboard contents. Host-returned references enter the canonical document and shared history. |
| Formatted paste | Native clipboard HTML retrieval; bounded original HTML-to-Markdown conversion for paragraphs/headings, nested lists, quotes, tables, links and inline formatting. Source paste retains plain Markdown bytes. Unsupported/empty/oversized HTML falls back to text. Scripts/resource-bearing elements are excluded; the converter never executes HTML or fetches resources. |
| Configuration/polish | New controls remain configurable, with separate feature flags. Active mark buttons highlight; task bullets are suppressed; table controls use an original painted icon; Rich and Source retain the existing history. |

The standalone includes a local attachment service. `--asset-root /explicit/root` selects its scope; otherwise a source/save file's parent is used, falling back to a temporary demo directory. It accepts files up to 16 MiB, imports with unique names into `assets/`, resolves only canonical paths inside its root and previews PNG files. It does not implement GlyphFolio's attachment broker. Hosts may provide additional bounded decoders through the same service.

Decoded adapter images are limited to 4,194,304 pixels and retained image data to 32 MiB, with at most 64 references considered per revision. GPU textures add memory, and upstream native clipboard decoding can allocate before adapter validation. These are component limits, not a whole-process memory guarantee. Asset resolution/import is synchronous in this first API; production hosts should supply cached resolution and plan asynchronous import. Imported files remain host-owned after undo; hosts own orphan cleanup and relocation when saving a note elsewhere.

## Try the feature fixture

```sh
cargo run --locked -p glyphfolio-editor-demo -- \
  --source-file examples/standalone-egui/fixtures/editing-parity.md
```

Use the Table menu for structural operations; Tab selects cells. Use the link icon for a URL or note link. Drop a file into the editor or use the Paste toolbar for a file-only clipboard. Ordinary native image paste is handled through egui's `PasteImage` event. File-only keyboard paste depends on the host emitting an event; the explicit Paste action supplies a reliable adapter entry point.

For another host, install `NativeViewState::set_asset_service`, retain `Editor` and `NativeViewState`, and render `EditorPanel`. Bare-widget hosts can call `begin_link_edit` / `show_link_dialog` and `paste_clipboard` themselves. Existing copy/cut acknowledgements, composition ownership and revision-tagged saving remain in place.

## Remaining parity and qualification work

- Native table cells now wrap and align using shaped geometry. Columns use equal widths with a minimum width; resizing, reordering, merged cells and spreadsheet-style rectangular selection are future work. Whole tables are shaped together. Nested/container tables and ragged structures retain source fallbacks.
- Wiki links currently accept simple labels; nested Markdown labels/embeds and application note resolution need further work. This increment is not full GFM dialect conformance: broader autolink behavior and extension-specific corpus coverage remain to be established.
- One resolved image per paragraph is presented. Additional image formats, multiple inline images, asynchronous loading, import rollback/cleanup and GlyphFolio attachment resolution need host work. No remote resource fetching is enabled by default.
- Formatted conversion is a bounded original clipboard converter, not a browser DOM implementation. More malformed-HTML, complex table/list and cross-application clipboard fixtures are needed. Real OS HTML/file/image clipboard roundtrips have not been requalified in this increment; event/provider tests establish adapter behavior only.
- Caret movement no longer reveals Markdown delimiters or changes Rich layout. Rich IME still temporarily reveals Source, and the existing large-styled-paragraph Source safeguard remains. Link dialogs reject stale edits, but more advanced nested-label edits need refinement. Unchanged link confirmation preserves exact source; destination edits retain escaped labels and title semantics.
- GlyphFolio's React/Tauri native hosting, SQLite saves, per-note history, sync revisions, attachment broker and plugin actions remain separate integration work. No replacement or memory-policy claim is made.
- Previously recorded VoiceOver and Windows/Linux runtime gaps remain follow-up work, not blockers for an honestly scoped preview.

See [the active roadmap](REPLACEMENT_ROADMAP.md) for delivery order and [the previous qualification report](PHASES_1_6.md) for historical native evidence. New evidence is recorded under `docs/evidence/editing-parity/`; do not apply older binary checks to the new executable.

## Continuation validation

The local workspace now passes **211 tests**, including the retained CommonMark corpus, new GFM/source-map cases, native event/provider paths, table operations and mixed-selection/link-fidelity regressions. Formatting, strict Clippy across all targets, warning-free rustdoc, native dependency-boundary checks and the standalone demo build pass. [Machine-readable results](evidence/editing-parity/validation.json) identify the current debug executable and scope the retained native captures separately. Logs are saved beside the report.

The continuation fixes consistent application of task/strikethrough formatting across mixed selections, exact no-op link confirmation, and label/title preservation when changing a link destination. Neither GlyphFolio application was edited. No package was published.

The subsequent [Rich editing increment](RICH_EDITING.md) supersedes the original table-grid and active-block-reveal behavior above. Its validation is recorded separately under `evidence/rich-stable/`.
