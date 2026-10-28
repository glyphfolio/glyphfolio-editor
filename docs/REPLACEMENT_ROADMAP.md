# Editing-first replacement roadmap

Updated 2026-10-28 following the owner's priority change and the GlyphFolio feature-gap review. This is the active delivery order; earlier phase reports remain historical evidence.

The immediate product is a useful native rich Markdown editor that can reach GlyphFolio editing parity. Accessibility and broader platform qualification continue as follow-up work and do not block an explicitly limited open-source preview. Preserve their existing implementation and report unverified support honestly. Document fidelity, safe transactions, undo and source mapping remain release requirements.

See [the first executable parity increment](EDITING_PARITY.md) for current behavior and explicit limits. The entries below now distinguish initial implementation from completed acceptance.

## Delivery order

| Order | Deliverable | Acceptance evidence | Current status |
| --- | --- | --- | --- |
| 1 | GFM and wiki-link grammar; strikethrough and interactive task lists | Explicit dialect tests alongside the unchanged CommonMark corpus; click/keyboard edits, Source/Rich switching and exact undo; wiki-link resolution is a host request | First implementation present; acceptance incomplete |
| 2 | Editable native tables | Insert a table, edit cells, navigate cells, add/remove rows and columns; alignment and escaped pipes survive deliberate edits and save/reopen; one shared history | Native cells and local editing regression checks complete; cross-platform qualification remains |
| 3 | Images and attachments | Host-owned resolve/import interface; native image presentation with bounded decoding/cache; paste/drop files through the host; relative references and unsupported assets remain editable without data loss | First implementation present; acceptance incomplete |
| 4 | Formatted paste | HTML-to-Markdown conversion for blocks, nested lists, tables and inline marks; safe links, no executable content or automatic remote fetching; explicit plain-text fallback; paste undoes atomically | First implementation present; acceptance incomplete |
| 5 | Rich editing polish | Link insertion/editing, visible formatting state, stable caret/selection across folded syntax, list/table interaction and composition behavior; light/dark/narrow-window review | Stable folding, pending marks and paragraph/table editing tested locally; native platform qualification remains |
| 6 | Independent public preview | Reproducible package and demo, configuration examples, feature/support matrix, licenses and clean-archive checks; disclose remaining limits | Foundation present; package/release checks pending |
| 7 | One controlled GlyphFolio integration path | Select an actual native hosting mechanism and demonstrate note open/edit/save/reopen, focus, resize and shortcuts against an isolated fixture | Not implemented; separate application changes require authorization |
| 8 | Everyday replacement qualification | SQLite saves, per-note history, incoming sync conflict policy, attachment broker and plugin editor actions; regression checks across required platforms | Not implemented |

Items 1–5 define the editing feature milestone. Preview preparation can run alongside them. A preview release and adoption in GlyphFolio are separate decisions: an independent library can be released before application integration is complete. Publication is not authorized by this roadmap.

## Implementation boundaries

- Build original commands and interaction behavior using the existing canonical document and history. Do not port CodeMirror's implementation.
- Keep the CommonMark reference configuration available. Add explicit GFM/wiki-link dialect policy and corresponding source maps instead of silently changing the existing reference tests.
- Preserve unedited source, including BOM, line endings, escapes, entities, table syntax and relative asset references. Parsing/rendering never rewrites the document.
- Expose asset import/resolution, link activation, persistence and plugin commands through public host interfaces. The editor must not depend on GlyphFolio's database, filesystem layout or application internals.
- New user-facing features must be represented in configuration, toolbar policy and keyboard handling. Existing functions stay available.
- Keep existing GlyphFolio applications unchanged while this workspace implements and tests parity. No access to the user's live notes is necessary for library work.

## Integration decision that must be demonstrated

The current deliverable is an egui widget. GlyphFolio currently hosts a React editor inside Tauri; replacing a React component with this widget is not a direct import. Before changing the application, demonstrate a concrete native hosting approach in an isolated host, including focus transfer, sizing, keyboard routing and shutdown. A separate demo window is useful evidence but does not establish embedded replacement.

The controlled application experiment must define ownership for note identity, revision-tagged saves, note switches, local history restoration, incoming sync changes, attachment operations and plugin actions. Reject or explicitly reconcile stale external revisions; never overwrite unsaved edits implicitly. Keep the independent editor engine intact.

## Follow-up qualification

Actual VoiceOver workflows, Windows/Linux runtime checks, extended script/font coverage, long soaks and accessibility performance remain tracked work. No unsupported platform or screen-reader claim follows from a macOS build or synthetic test. Continue preserving and regression-testing the existing input/accessibility paths as editing features evolve.

Large styled Rich paragraphs currently have a Source safeguard; Rich composition currently reveals Source; whole-document accessibility geometry can be expensive. These remain visible limitations until new implementation and measured evidence replace them. Standalone measurements do not establish GlyphFolio's whole-application memory target, and native editor hosting alone does not remove its surrounding WebView.

## Definition of progress

Record each deliverable as implemented, tested locally, demonstrated natively, or qualified on a named platform. Do not treat grammar parsing alone as interactive feature completion, a rendered table as editable-table completion, or a host interface as working application integration.
