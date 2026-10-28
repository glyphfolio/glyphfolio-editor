# glyphfolio-editor — Implementation Plan

**Mission:** Build an original, embeddable, native Rust rich Markdown editor, owned and maintained by GlyphFolio, with egui as its first UI adapter and no browser, WebView, JavaScript, or Node runtime requirement.

**Status:** Approved architectural baseline, with delivery priorities revised on 2026-10-28. The active [editing-first replacement roadmap](docs/REPLACEMENT_ROADMAP.md) prioritizes GFM/wiki links, tables, assets, formatted paste and Rich editing polish before broader qualification. Accessibility and platform gates remain valid support criteria, but are not prerequisites for an explicitly limited open-source preview. Earlier phase ordering and deferred-feature scope below are superseded by that roadmap where they conflict. See `docs/PHASES_1_6.md` for historical implementation and qualification evidence. Existing GlyphFolio applications remain unchanged; no unexecuted gate is marked passed.

## 1. Architectural direction and ownership

`glyphfolio-editor` is a completely greenfield editor project. GlyphFolio will implement and own its document model, transaction engine, undo/redo, selections, caret navigation, editing commands, Rich/Source projections, Markdown source mapping, hit testing, layout orchestration, caching, virtualization, and editor interaction behavior.

We will not adopt, fork, wrap, extract, copy, port, or adapt an existing editor implementation. This excludes Leaf, Ferrite, CodeMirror, Markion, Velotype, `egui_richedit`, `egui_pop_editors`, and equivalent editor engines or components as implementation foundations.

Existing editors may be studied for observable behavior, interoperability, accessibility expectations, and regression scenarios. CodeMirror remains a behavioral reference and regression oracle; its editor source is not a porting source. Document any intentional behavioral differences rather than treating every CodeMirror behavior as a public compatibility obligation.

### 1.1 What we build and what we may reuse

| Component | Implementation policy |
| --- | --- |
| Canonical document, transactions, change mapping, history | Original project implementation |
| Selections, caret movement, commands, pointer and keyboard interaction | Original project implementation |
| Rich/Source projections and Markdown source maps | Original project implementation |
| Block layout orchestration, hit testing, virtualization, caches | Original project implementation |
| Text storage | A low-level library such as Ropey is permitted |
| CommonMark parsing | An established parser such as pulldown-cmark is permitted |
| Unicode segmentation, bidi algorithms, line breaking | Established low-level libraries are permitted |
| Font discovery, shaping, rasterization, glyph rendering | Established low-level libraries are permitted |
| Native window, events, and painting | egui/eframe and their platform backends are permitted |
| IME, clipboard, editable accessibility | Implement our adapter using toolkit and platform APIs |

Reusing low-level infrastructure does not authorize reusing its editor controller. For example, COSMIC Text or Parley may supply shaping/layout primitives, but their ready-made editor interaction, selection, history, or command components must not become our engine. Likewise, egui font/glyph APIs are permitted; `egui::TextEdit` must not supply this editor's document editing behavior. Ordinary egui text fields elsewhere in demo controls are outside that restriction.

Dependency and contribution reviews must check both declared dependencies and the APIs actually used. New engine code must be original project work; preserve attribution and notices for permitted infrastructure and assets.

### 1.2 Three governing principles

1. **Independent and embeddable.** Engine and layout contracts have no dependency on GlyphFolio application code, egui, eframe, Tauri, a DOM, a WebView, or an application database. egui is the first adapter.
2. **One original editing architecture.** One canonical Markdown document feeds both presentations and one transaction/history engine.
3. **Open source from the beginning.** Design a public Rust library with tests, documentation, examples, and permissive `MIT OR Apache-2.0` licensing for original project code.

### 1.3 Independence from the current GlyphFolio migration

Develop and qualify this library separately from the existing GlyphFolio application and its migration/release work. This plan does not modify the current Tauri/React/CodeMirror application, its editor, plugins, storage, synchronization, or release gates.

Build standalone native demos and a generic Rust host example first. Any later GlyphFolio adoption requires a separately scoped integration design and acceptance decision. An egui widget embeds naturally in an egui host; embedding it in an existing web-based shell is a separate native-hosting problem. Creating this library does not by itself remove a host application's WebView or JavaScript runtime.

The project is valuable as an independent editing framework. Resolving a particular GlyphFolio memory symptom is not its sole justification or a promised outcome.

## 2. Scope and first deliverable

The next engineering milestone is the document and transaction engine alongside a small executable native feasibility prototype. Do not postpone text shaping, native input, or editable accessibility until the full editor exists.

The first end-to-end editing milestone, M1, follows the foundational and feasibility gates below.

### 2.1 M1 scope

- Valid UTF-8 Markdown storage, immutable snapshots, validated transactions, revision-safe history, and explicit host change notifications.
- Source editing and a Rich presentation of paragraphs, H1–H6 headings, emphasis/strong, inline code, inline links, unordered/ordered lists, blockquotes, and fenced code.
- A single logical selection spanning blocks; grapheme/word navigation, visual line navigation, pointer placement, selection dragging, and preferred-X tracking.
- Plain-text clipboard copy/cut/paste and basic formatting commands with one undo history.
- Native composition, candidate positioning, and editable accessibility for the explicitly qualified initial platform/backend combination.
- A standalone egui demo and a generic host example with save acknowledgement, dirty state, and stale-result handling.
- Exact document fidelity, edit/undo/save/reopen tests, parser/projection tests, and measured interactive workloads.

M1 rich rendering is a documented subset of Markdown. Recognize and preserve other syntax without silently deleting, canonicalizing, or misrepresenting it. Use an editable source presentation for unsupported or ambiguous constructs. Parser conformance and supported rich interactions are separate claims.

### 2.2 Deferred capabilities

Defer rich table editing, binary asset management/image rendering, real-time collaboration, LSP integration, general editor plugins, multi-cursor editing, and mobile touch editing. Preserve deferred syntax and keep it accessible in Source presentation.

Formatted HTML paste is a separate capability gate and is not implied by plain-text clipboard support. Broader platform acceptance is also separate from the initial native feasibility result.

## 3. Workspace and dependency boundaries

Use a Cargo workspace with four engine/adapter crates and isolated demo packages. Directory names below are provisional; published names should be namespaced and availability checked before publication.

| Directory / proposed package | Responsibilities | Dependencies and boundary |
| --- | --- | --- |
| `crates/editor-core` / `glyphfolio-editor-core` | Document, snapshots, coordinate contracts, transactions, selection state, commands, history | Low-level storage/Unicode; no UI or Markdown parser dependency |
| `crates/editor-markdown` / `glyphfolio-editor-markdown` | Parse projection, syntax metadata, source/display mapping, reference dependencies, Markdown-aware commands | Core and permitted parser/Unicode libraries; no UI |
| `crates/editor-layout` / `glyphfolio-editor-layout` | Layout contracts, block orchestration, caret/selection geometry, height index, virtualization, cache policy | Core and projection data through UI-independent types; no egui types |
| `crates/editor-egui` / `glyphfolio-editor-egui` | Custom widget/controller adapter, rendering, native input, clipboard bridge, AccessKit integration | Core, Markdown, layout, egui; implements backend traits |
| `examples/standalone-egui` | Native demo and feasibility surfaces | eframe plus the public adapter API |
| `examples/host-integration` | Generic Rust host ownership, save, focus, lifecycle, and commands | Public engine/adapter API; no GlyphFolio application imports |

`editor-layout` may consume a projection contract from `editor-markdown` or a small shared UI-independent contract module. Select and document one direction before creating types; avoid circular dependencies.

The egui-backed shaping/painting implementation belongs in `editor-egui`. A trait inside an egui-free crate must not expose egui `Galley`, `Ui`, `Rect`, `FontId`, texture handles, or other concrete backend types. Keep opaque backend resources in the adapter and return independent metrics and caret data to the layout layer.

Initially evaluate Ropey, pulldown-cmark, unicode-segmentation, appropriate Unicode/bidi/line-break facilities, and low-level egui, COSMIC Text, or Parley shaping APIs. These are candidates, not an accepted dependency set. `unicode-width` describes text-column widths and is not a proportional-font shaping engine. Add conveniences such as smallvec/serde only when their use is justified; serialized internal state is not automatically a public format.

Pin a compatible Rust/toolkit/backend set for the feasibility work. Record exact versions/features in its evidence and lock executable demo dependencies. Do not copy API sketches from floating `latest` documentation into an assumed compiling contract.

## 4. Public embedding contract

Expose capabilities comparable in purpose to an embeddable editor API, while retaining freedom to change implementation details before stabilization. No CodeMirror API or extension compatibility is promised.

The public API must let a host:

- Create an editor session from Markdown and obtain immutable revision-tagged snapshots.
- Submit validated changes or commands, set/query selections, and undo/redo.
- Observe committed changes, history availability, composition state, and errors.
- Attach view state and choose Rich or Source without replacing the document.
- Read exact Markdown for persistence, acknowledge a particular saved snapshot, and reject stale asynchronous results.
- Provide theme/font settings, clipboard services, link-opening callbacks, and accessibility/platform services through bounded interfaces.

Keep Ropey, parser ASTs, source-map storage, and cache internals behind the API. Offer bounded text access, chunk iteration, or explicit snapshot export; avoid requiring full-document strings on each frame or keystroke. Any method names in design discussions remain illustrative until the contracts are reviewed and tested.

Separate document/session ownership from per-view state. Document and history survive widget recreation. Each view owns its focus, projection settings, viewport, reveal state, and geometry; composition has exactly one input owner. A split-view demo must not create two independent document authorities.

## 5. Canonical document, coordinates, and persistence

### 5.1 Document invariants

- Exactly one authoritative valid UTF-8 Markdown buffer exists per document session.
- Parsed structures, rendered strings, and layout are disposable derived data; never serialize an AST to persist an ordinary edit.
- A committed content change advances a monotonic revision. Undo and redo also advance it; mode, selection, and viewport changes do not.
- Snapshot identity includes document/session identity and revision. Background parse/layout/save results must match their intended identity before publication.
- Preserve unchanged source exactly, including delimiter choice, escapes, spacing, line endings, BOM, and trailing newline. Reject unsupported/invalid input encodings explicitly rather than decoding lossily.
- Initial storage policy is valid UTF-8; arbitrary binary or legacy-encoding roundtripping is outside M1.

Cheap shared storage snapshots are permitted; derived full strings must have explicit ownership and lifetime. Store no second independently editable rich document.

### 5.2 Coordinate types

Define distinct types for source byte offsets/ranges, Unicode scalar indices, logical source lines, display positions, and backend text-run positions. Use a separately named UTF-16 coordinate only where a platform boundary requires it.

For any public line/column representation, define the column as Unicode scalar count from the logical line start. Grapheme movement and visual columns use separate operations/types; a column must not sometimes mean bytes and sometimes graphemes.

Validate source bounds and UTF-8 boundaries at every mutation boundary. Ropey byte-to-character conversion is not itself proof of a valid requested byte boundary. Check explicitly before converting to its edit coordinates. Segment graphemes across rope chunks with the required context; chunk-local segmentation is insufficient.

Align Markdown logical-line semantics with parser requirements while preserving raw LF/CRLF bytes. Treat CRLF as one user-facing line break and define deletion/navigation at its boundary. Ropey's configurable line recognition must not silently determine a different Markdown grammar.

A selection retains anchor/head direction. Caret positions need documented affinity for folded syntax and soft-wrap boundaries; selection endpoints may require independent affinities. Bidi visual ordering must not be inferred from source byte ordering.

### 5.3 Save and dirty-state semantics

The host owns storage and save scheduling. It saves a named snapshot and acknowledges that snapshot; a late acknowledgement must not mark newer edits clean or save into a different document session. Define content-state equivalence when undo returns to a saved state; a monotonic revision alone is not an adequate dirty-state test.

Plain no-op load/save preserves the accepted input bytes. Save must not include provisional composition. Test host acknowledgement, close/reopen, failed-save recovery, and document identity in the generic example without importing GlyphFolio's storage system.

## 6. Transactions, history, and commands

### 6.1 Atomic changes

Each submitted transaction identifies its target session and expected revision. Every change range refers to the same pre-transaction snapshot. Define deterministic ordering and reject ambiguous overlapping changes, including coincident insertions unless an explicit ordering rule exists.

Before mutation, validate every range, UTF-8 boundary, revision, and resulting selection. Apply the transaction atomically, using descending source offsets or an equivalent change-set algorithm. Invalid input leaves text, revision, selection, history, and notifications unchanged.

The result provides a new snapshot, a change map, committed selection, and exact inverse changes. Inverse ranges are expressed in the post-transaction coordinate space. Offset mapping includes explicit association at inserted/deleted boundaries.

### 6.2 Revision-safe history

History stores forward/inverse operations and before/after selections, independent of the obsolete revision against which the original transaction was submitted. Materialize undo and redo transactions against the current revision and session.

For example, after edits `0 -> 1 -> 2`, undo produces revision 3. The next undo must operate at revision 3; it cannot replay an inverse still expecting revision 1. General collaboration/rebasing is outside M1, but this ordinary history case is mandatory.

Coalesce typing only when origin, adjacency, direction, selection continuity, time window, and explicit grouping permit it. A 400 ms window is an initial tunable parameter, not the grouping rule. Break groups at navigation/selection discontinuities, paste, formatting, host boundaries, and composition transitions. One committed composition is one deliberate history step. A new committed edit after undo clears the redo branch.

Set and test history memory budgets, retention/truncation policy, and snapshot lifetimes. A mode switch does not clear history. Programmatic edits must explicitly opt into the documented history behavior; no invisible mutation path is allowed.

### 6.3 Commands and interaction

Implement our own insertion/deletion, grapheme and word navigation, visual navigation, selection extension, formatting, indentation, and block commands. Markdown-aware commands live above the generic transaction engine and produce source edits.

Specify behavior for empty selections, existing delimiters, nested marks, links, crossing block boundaries, Enter/list continuation, Backspace at block starts, and list/code Tab behavior. Avoid modifying unrelated source while formatting a selection. Navigation and deletion must have explicit rules for hidden delimiters and transformed text; do not leave a caret trapped in invisible syntax.

Use platform-aware keymaps. Validate macOS Cmd/Option behavior and Windows/Linux Ctrl/Home/End behavior, including Shift selection and redo conventions. Hosts may override bindings; commands remain callable without a keyboard.

## 7. Markdown parsing and source projection

### 7.1 Correctness baseline

Use an established CommonMark parser as the initial semantic reference. Build a measured full-parse path before implementing incremental optimization. Capture exact parser version/options, dialect extensions, and logical-line policy.

pulldown-cmark event ranges can inform source mapping, but are not a complete lossless token stream or an incremental editing API. Its contiguous input requirement must be accounted for when materializing rope snapshots; do not hide full-document copying in a keystroke path.

Test the official CommonMark corpus for the selected grammar, then add source-preservation, projection, malformed-input, and editing cases. A hand-picked set of 100 cases does not establish conformance. Rendering only M1 constructs is permitted; losing unsupported source is not.

### 7.2 Incremental invalidation

Do not promise worst-case `O(dirty lines)` CommonMark updates. A fence edit can affect blocks through EOF; list/quote context and reference definitions can affect unedited content.

When optimization is justified by measurement:

1. Map the committed source changes to candidate dirty regions.
2. Resume from a safe parser checkpoint with sufficient container state.
3. Rescan until source position and parser state converge with valid downstream state.
4. Maintain reference-definition dependencies and invalidate affected inline projections, including offscreen references.
5. Fall back to wider or full parsing when locality cannot be established.
6. Differentially compare incremental results against a fresh full parse after generated edits.

Visible-block inline work may be lazy, but must consume correct surrounding/global reference information. Keep byte ranges, parser generations, and document identity explicit. Reject stale worker results. A broad structural edit may legitimately require broad parsing; keep it scheduled and observable rather than returning incorrect local results.

### 7.3 Piecewise source/display mapping

Replace the original single `source_range`/`content_range` span model with independent nested style runs and ordered projection segments:

| Segment | Meaning | Required mapping behavior |
| --- | --- | --- |
| Copy | Visible content follows an exact source slice | Convert through validated text boundaries |
| Hidden | Source syntax has no visible text | Associate boundaries using documented affinity/reveal policy |
| Transform | Visible text differs from the source spelling | Retain explicit source/display boundary associations and editing rules |

Cover entities such as `&amp;`, escapes such as `\*`, code-span normalization, soft/hard breaks, nested emphasis inside links, and repeated list/quote prefixes. Source ranges and display ranges are different coordinate spaces; visible character indices cannot simply be added to a source byte start.

Hidden syntax creates many-to-one mappings. Define canonical legal caret stops and equivalent boundary positions rather than claiming a universal bijection. For ambiguous/transformed constructs, reveal source or apply an explicitly defined whole-segment edit; preserve untouched bytes.

Keep syntax metadata immutable for a document snapshot. Per-view fold/reveal policy depends on selection and composition and has its own generation. Implement active-block reveal with anchored reflow; it can change wrapping and height and must not be described as reflow-free. Stabilize the active projection during composition.

## 8. Layout orchestration, geometry, and virtualization

### 8.1 Backend responsibilities

We own block structure, flow, margins, indentation, adornments, visual line navigation, hit testing, and selection geometry. Permitted font libraries supply glyph shaping, cluster data, fallback, and rasterization. Their editor controller is not used.

Define a UI-independent shaping contract carrying text/style input, glyph-run metrics, cluster/source associations, legal caret stops, bidi direction, line-break results, and any necessary opaque backend key. Choose one owner for line breaking and wrapping; do not independently reimplement wrapping against different metrics from those used to paint.

Qualify the backend before freezing this contract. Switching from egui galleys to another shaper can affect painting, atlases, fallback fonts, clusters, hit testing, selection, and accessibility. It is not assumed to be a small later change.

### 8.2 Stable identity and caches

Use stable block IDs, not current vector indices. Track block content generation separately from document revision and layout generation. Cache keys include width, scale, font/typography generation, projection/reveal state, and any theme properties that affect geometry.

Map source locations through changes without forcing every unchanged block to be reparsed or reshaped. Define invalidation for split/merge, reference changes, font replacement, resize, mode switch, and composition. Bound retained galleys, raster resources, derived strings, and history/snapshot references.

### 8.3 Virtualized document geometry

Maintain a searchable height index over logical blocks, with estimated heights for unmeasured content, overscan, and bounded layout work per frame. Resolve visible regions in logarithmic/indexed time where practical. Correct estimates as blocks are measured and preserve a source anchor plus viewport-relative displacement to prevent jumps.

Provide an explicit strategy for very large single paragraphs and fenced code blocks: chunked line layout, cached line ranges, or another measured bounded-work approach. Whole-block virtualization alone does not establish responsiveness for these cases.

Exact total height may become available progressively. Expose estimate state where needed; avoid inventing precise geometry for unmeasured content. Offscreen caret navigation and accessibility actions must schedule/resolve geometry and scroll the target into view.

### 8.4 Hit testing and selection

Hit testing maps a view point to a legal display caret and then to source position/affinity. Cursor geometry performs the reverse projection, using the same shaped clusters and wrapping used for painting. Support empty blocks, tabs, newlines, ligatures, folded/transformed segments, and bidi runs.

Track preferred X for vertical navigation. A logical selection can produce multiple visual rectangles, especially across wraps and bidi runs. Selection direction remains logical session state.

The geometry property is legal caret -> representative hit-test point -> equivalent caret, with documented fold/wrap/bidi equivalence and tolerance. A thin caret rectangle cannot contain every point inside a wide glyph; that original gate is removed.

## 9. Native adapter contracts

### 9.1 Event ownership and rendering

The custom egui widget routes focused keyboard, pointer, and platform events into our commands/transactions. Ensure one owner consumes each event; the native feasibility prototype must exercise our controller rather than delegating text editing to an existing widget.

Paint block backgrounds/adornments and selections, text, carets, and composition overlays in a defined order. Schedule caret-blink repaint only while needed and reset blink on input. Idle immediate-mode frames must not trigger full parse/layout work.

### 9.2 IME and native input

Use APIs from the selected pinned toolkit version. The earlier `OutputEvent::Ime(ImeOutput::Position(...))` sketch is not an accepted API. In the checked egui 0.36.2 reference, candidate geometry uses `PlatformOutput.ime` and `IMEOutput`; revalidate against the version actually selected.

Define ordinary text input, composition start/preedit/update/commit/cancel, preedit selection, replacement of an existing selection, surrounding-text deletion, interruption, and focus transfer. Map toolkit character units to source coordinates explicitly. Maintain preedit in a composition overlay tied to an anchor and snapshot; do not persist it as committed Markdown.

Preserve one deliberate history step on commit and no edit on cancellation. Handle duplicate text/commit delivery according to the backend contract. Defer mode change and session closure during active composition until it commits or cancels; do not discard it silently. Tag callbacks by session so late input cannot enter another document.

Derive candidate geometry from final painted caret/layout coordinates, with correct logical/physical scaling, scroll, clipping, window placement, and monitor changes. Verify real native input; synthetic events alone do not establish platform correctness.

### 9.3 Clipboard and links

Use host/toolkit plain-text copy/cut/paste events through one clipboard owner. For M1, ordinary copy exports the selected canonical Markdown source range; specify behavior at hidden/transformed boundaries. Cut deletes only after successful clipboard publication, and paste is one atomic history step.

Formatted HTML import/export requires a separate format-capable service, an inert parser/converter, explicit supported formats, and platform tests. A dependency on arboard alone does not prove HTML paste support. Conversion must not execute markup or fetch resources. Implement this in a later capability milestone unless separately brought into scope.

Links produce host callbacks after an intentional modifier/action. The host controls allowed schemes and actual launching; plain clicks remain editing interactions. No renderer starts network activity merely by parsing a document.

### 9.4 Editable accessibility

Define an editable semantic tree with focus, document/text input roles, stable logical identity, text runs, character boundaries and geometry, selection state, and change updates. AccessKit endpoints refer to text-run nodes and character indices; implement explicit mapping to/from source and projection positions.

Handle incoming accessibility selection, replacement, navigation, focus, and scroll requests through our transaction/command engine. Static heading/text announcements alone are not editable accessibility acceptance.

Reconcile virtualization with whole-document reading and offscreen navigation. Document how stable text-run IDs relate to reflow and how folded syntax is spoken or revealed. Test actual screen-reader read/navigate/select/replace/undo operations, including content initially outside the viewport.

## 10. Early feasibility gates

Record each gate as NOT RUN, PASS, PARTIAL, or FAIL, with exact commit/dependencies, OS/backend, hardware/display settings, procedure, observations, and remaining limitations. Documentation/API availability is not a runtime PASS. Partial support must be scoped explicitly; do not promote it as broad Unicode or platform acceptance.

| Gate | Small executable proof | Acceptance and consequence |
| --- | --- | --- |
| F0: ownership and dependency boundary | Minimal workspace and API/dependency inspection | Core/Markdown/layout build without egui or host application dependencies; no existing editor engine or controller is used |
| F1: transactions and fidelity | Headless engine harness using original project code | Atomic multi-edit changes, invalid-input refusal, current-revision undo/redo, exact byte restoration, selection mapping, immutable snapshots |
| F2: shaping and geometry | Custom native surface with a few styled runs and our hit testing | Latin ligatures, combining marks, CJK, Indic samples, emoji/ZWJ, mixed Arabic/Hebrew/Latin, fallback fonts, wraps, and caret/selection geometry are tested; backend gaps are resolved or scope decisions recorded before expansion |
| F3: native composition | Custom focused surface connected to our transaction/composition model | Real Japanese/Chinese input, replacement, cancel/commit, one-step undo, scrolling, scaling, focus transfer, mode-change deferral, and candidate placement work on the initial supported platform |
| F4: editable accessibility | Small multi-block editable tree connected to our commands | A real screen reader reads, navigates, selects, replaces, undoes, and reaches offscreen text; input roles and source/semantic mapping are verified |
| F5: host and clipboard lifecycle | Generic native host, isolated save service, second application for clipboard interchange | Correct focus/event ownership, exact plain-text copy/cut/paste, revision-tagged acknowledgement, composition-safe close, and stale-result rejection |

F1 and the F2–F4 prototypes progress together. Use macOS as the first local native proof, then execute the supported Windows and Linux backend matrix. A macOS result does not establish NVDA/Windows or Orca/Linux behavior. Choose and record the supported Linux display/input session(s).

Do not build broad Rich features around a backend that has failed the required native-input, shaping, or accessibility gate. Address its concrete gap, change the permitted low-level backend, or make an explicit supported-scope decision. No automatic substitution of another editor engine is allowed.

## 11. Updated implementation order

The phase sequence is dependency-driven. Phase 1's two tracks run alongside each other; neither waits for a complete Rich editor.

| Phase | Deliverable | Exit gate |
| --- | --- | --- |
| 0: contracts and public workspace | Ownership policy, four crates, license files, public/internal boundaries, coordinates, transaction/projection contracts, initial CI | F0; contracts reviewed; all capability claims marked planned |
| 1A: document/transaction engine | Rope storage, snapshots, atomic changes, inverse mapping, selections, history, basic commands, save identity | F1 with property tests and bounded history policy |
| 1B: early native feasibility | Original custom surface using permitted shaping/platform APIs, our caret/selection/input, IME and editable accessibility | F2–F4 on the first platform; backend decision and known limitations recorded |
| 2: original Source editor | Connect accepted native surface to the engine; pointer/keyboard interaction, plain clipboard, host lifecycle | F5 plus working native edit/undo/save/reopen; repeat input/accessibility checks on the connected engine |
| 3: Markdown reference and projection | Full-parse correctness path, dialect policy, piecewise source maps, nested style runs, malformed/unsupported source fallback | Official grammar cases and editor-specific projection/editing tests pass within declared scope |
| 4: small Rich editing slice | Paragraph, heading, emphasis/link/code interactions, active-block reveal, Rich/Source toggle with shared history | Exact source edits, stable source anchors, legal caret behavior, composition-safe toggles, editable semantics |
| 5: measured incrementality and virtualization | Safe parser checkpoints, reference invalidation, stable block IDs, height index, bounded layout/caches, long-block strategy | Incremental/full differential tests and measured large-document/input workloads |
| 6: M1 completion and platform qualification | Lists, quotes, fences, full declared commands, toolbar, standalone demo and generic host example; supported-platform matrix | M1 interaction suite, native input/screen-reader evidence, fidelity and resource reports |
| 7: hardening and public preview | Fuzzing, adversarial edit/parser cases, soak tests, packaging/docs, release/contribution policy | Reviewable evidence, declared API/support limits, reproducible packages, no unqualified readiness claims |
| 8: separately scoped adoption and extensions | Future GlyphFolio adapter decision; tables/assets/formatted paste and other extensions as approved | Separate feature and application acceptance; current GlyphFolio migration is not a dependency |

Property tests, dependency review, and input/accessibility regression checks begin in the relevant early phase and continue throughout. Phase 7 broadens hardening; it is not the first point at which fuzzing or correctness tests exist.

Phase 0 also specifies a generic public API design fixture in `docs/PUBLIC_API_FIXTURE.md` and `examples/host-integration`. It covers document creation, transactions, change subscription, Rich/Source view preferences, and revision-tagged save acknowledgement. Phase 1A/2 makes the fixture compile against the original engine without host-application internals.

## 12. Verification and performance criteria

### 12.1 Correctness gates

| Invariant | Verification | Acceptance |
| --- | --- | --- |
| Original independent engine | Dependency graph, API use, contribution/provenance review | No existing editor implementation is used as dependency or copied/ported code |
| Transaction atomicity | Invalid revisions, ranges, overlaps, selections, and Unicode boundaries | Entire transaction refused without partial state/notification changes |
| History reversibility | Generated multi-change edits, undo/redo interleavings, grouping, divergent edits | Exact original bytes and documented selection states restored; revisions stay monotonic |
| Persistence fidelity | No-op load/save and edit/undo/save/reopen with BOM, mixed endings, whitespace, malformed Markdown | Exact accepted UTF-8 bytes preserved except deliberate edits |
| Unicode integrity | Scalar/grapheme/chunk-boundary cases, combining marks, ZWJ sequences | No invalid slicing, panic from user input, or unintended grapheme splits by user commands |
| Parser correctness | Official corpus, extension-specific cases, full/incremental differential tests | Selected grammar behavior agrees with reference; unsupported rich syntax is source-editable |
| Projection safety | Entities, escapes, nested marks, normalized code spans, hidden boundaries | Deterministic caret/edit mapping; unchanged source retained |
| Mode transition invariance | Toggle while selected, scrolled, editing, and composing | Same committed bytes, selection endpoints, history and logical content anchor; composition follows explicit deferral policy |
| Geometry consistency | Legal caret/point equivalence, wraps, bidi, folds, empty blocks | Consistent legal caret stops within documented tolerance; selection matches painted text |
| Native input/accessibility | Synthetic regression plus real OS IME and screen-reader procedures | Accepted behavior recorded per supported platform/backend; no synthetic-only OS PASS |
| Runtime independence | Cargo/features, linked dependencies, packaged contents and launched process inspection | Library/demo requires no browser, WebView, JavaScript, or Node runtime; symbol searches are supplementary |

Full undo compares bytes/content state, not equality of revision counters. Mode switches preserve source anchors rather than identical pixel scroll offsets across different geometry.

### 12.2 Workloads and measurements

Measure core, parser, projection, layout, and native UI separately, then measure end-to-end behavior. Record release build settings, hardware, OS, backend, fonts, scale, cold/warm state, fixture identity/hash, and the exact operation population.

Include 100 KiB notes, 1/5/10 MiB documents, 10,000+ short lines, a huge single paragraph, a huge fence, nested containers, distant reference links, Unicode-heavy text, large paste/delete/undo, width/font changes, and repeated document/view replacement. The 500-note GlyphFolio workload may be reproduced using approved isolated or synthetic data as an optional host comparison; it is not an engine dependency or permission to access a live library.

Record input-to-visible-update latency, frame/layout/parse percentiles and worst cases, work per frame, startup/open time, snapshot/save/export costs, undo memory, retained caches, font/glyph resources, process memory, and temporary peaks. Separate allocator/heap observations from OS physical-footprint accounting and include attributed helper/GPU resources where relevant.

Use a 60 Hz/approximately 16.7 ms frame budget as an initial interactive target on recorded reference hardware, then define p95/p99 and worst-case budgets from the small executable slice before qualifying M1. FPS alone does not establish input responsiveness, bounded parsing, or memory stability.

The existing GlyphFolio targets of warm 100 KiB open p95 <150 ms, local save acknowledgement p95 <250 ms, and 256 MiB whole-application memory belong to that application's recorded workload. Retain them as optional future host comparison targets with unchanged accounting; do not claim the library meets them or apply them indiscriminately to 10 MiB notes. Do not derive promised savings from removing a WebView.

Current GlyphFolio diagnostics show lifecycle/integration can affect retained memory. This project's plan neither diagnoses all existing growth nor authorizes changes to its application. Compare released/qualified native artifacts under matched workloads before asserting an improvement.

## 13. Open-source and release readiness

- Supply `LICENSE-MIT` and `LICENSE-APACHE` and declare `MIT OR Apache-2.0` for original project code. Preserve upstream and bundled font/asset notices; verify their terms rather than assuming our license relabels them.
- Publish an ownership/contribution policy, contributor guide, code of conduct, issue templates, and security-reporting contact. A DCO or CLA is a deliberate governance choice, not an automatic requirement of this plan.
- Document public API versus internals, MSRV, supported dependency/backend versions, supported platforms, dialect/rendering scope, accessibility/input limitations, and pre-1.0 compatibility policy.
- Include rustdoc examples, a headless engine example, standalone native demo, generic host example, architecture notes, and reproducible validation procedures.
- CI runs formatting, appropriate clippy checks, tests and docs across supported OS/toolchain combinations. Check engine crates without egui/default UI features and test supported feature combinations. Verify that application dependencies do not leak into engine crates.
- Run property/differential tests in normal development; use reproducible seeded fuzzing/soaks and preserve minimized failures. Keep timing benchmarks controlled and separate from noisy shared-CI performance claims.
- Validate packages from clean archives, required files/notices, example builds, versioned docs, and namespaced crate availability before publishing. Publication and production adoption are separate release actions.

Do not estimate product completion from the earlier 4–6-quarter or 15–20-person-month figures. Re-estimate after the transaction engine, native feasibility gates, and first connected editing slice provide implementation evidence.

## 14. Immediate engineering milestone

After this plan, the first implementation increment is:

1. Create the public workspace, license/documentation baseline, dependency policy, and independent core types.
2. Implement document snapshots, atomic transactions, change maps, revision-safe history, and exact fidelity/property tests.
3. Alongside that engine work, build the smallest original native surface that can demonstrate our caret/selection logic, permitted shaping backend, IME, and editable accessibility.
4. Record F0–F4 evidence, resolve backend/contract gaps, and connect the accepted surface to the engine before broadening Rich Markdown features.

This document does not record those steps as completed and does not authorize replacing GlyphFolio's current editor or altering its migration.

## 15. Technical references

References identify permitted infrastructure and platform contracts. Existing editor implementations remain behavioral references only.

- [CommonMark 0.31.2 specification and examples](https://spec.commonmark.org/0.31.2/)
- [Ropey storage, coordinate and line-break documentation](https://docs.rs/ropey/latest/ropey/)
- [unicode-segmentation GraphemeCursor](https://docs.rs/unicode-segmentation/latest/unicode_segmentation/struct.GraphemeCursor.html)
- [pulldown-cmark parser and offset iterator](https://docs.rs/pulldown-cmark/latest/pulldown_cmark/struct.Parser.html)
- [egui LayoutJob formatting contract](https://docs.rs/egui/0.36.2/egui/text/struct.LayoutJob.html)
- [egui PlatformOutput IME contract](https://docs.rs/egui/0.36.2/egui/struct.PlatformOutput.html)
- [egui IMEOutput geometry](https://docs.rs/egui/0.36.2/egui/output/struct.IMEOutput.html)
- [AccessKit text positions and run-relative coordinates](https://docs.rs/accesskit/latest/accesskit/struct.TextPosition.html)
- [COSMIC Text low-level shaping/layout infrastructure](https://github.com/pop-os/cosmic-text)
- [Parley low-level text layout infrastructure](https://github.com/linebender/parley)
- [Arboard clipboard facilities and platform behavior](https://docs.rs/arboard/latest/arboard/struct.Clipboard.html)

Floating documentation links are discovery references. Record pinned versions and inspect the APIs actually selected during implementation; these links do not establish dependency acceptance or runtime qualification.
