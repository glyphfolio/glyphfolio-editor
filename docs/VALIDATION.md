# Initial implementation and feasibility record

Historical report: its Rust 1.95 declaration is superseded. The preview now requires Rust 1.99.1, matching the locally tested toolchain; no older-toolchain compatibility is claimed.

For the latest work see [Phases 1–6](PHASES_1_6.md). Historical record of the first increment. See [Phase 1B](PHASE1B.md) for subsequent shaping, input, accessibility, clipboard, and toolkit changes; the results and binary identities below apply only to that initial build.

Scope: approved Phase 0, Phase 1A, and initial Phase 1B work. The current GlyphFolio application and migration are outside this implementation. Tests use synthetic document content.

## Evidence discipline

Builds and simulated toolkit input establish only the contracts exercised. They do not establish real Japanese/Chinese input, candidate-window placement, VoiceOver editing, Windows/Linux behavior, complete bidi/shaping, virtualization, or performance acceptance. None of those capabilities may be inferred from a screenshot or an AccessKit tree.

## Environment

- Initial local host: macOS arm64.
- Installed compiler: rustc 1.99.1 (58cf8f9c3, 2026-10-13); Cargo 1.99.1.
- Declared workspace floor: Rust 1.95, matching the selected native toolkit manifests. Minimum-toolchain and other-OS CI runs are not local evidence.
- Native toolkit evaluation candidate: egui/eframe 0.36.2 and its compatible AccessKit dependency. This is a candidate for F2, not an accepted shaping solution for every script.
- Engine: Ropey 1.6.1 for storage and unicode-segmentation 1.13.3 as low-level infrastructure. No existing editor engine/controller is an implementation dependency.

## Gate status

| Gate | Status | Evidence / remaining work |
| --- | --- | --- |
| F0: independent original workspace | PASS locally | Four crate boundaries, permissive licenses, local repository, public host fixture, and native dependency checks; CI on other toolchains/OSs is not run |
| F1: transactions and fidelity | PASS for initial engine | 27 core tests, including three property functions with 160 generated cases each; independent String/history-model comparison and 15 adversarial regressions |
| F2: shaping and geometry | PARTIAL; coverage failures observed | Legal-grapheme geometry tests and visible native render pass; the selected font/backend configuration has missing glyphs and emoji/bidi gaps; full gate has not passed |
| F3: native IME | PARTIAL; real OS input NOT RUN | Focused synthetic event routing, provisional text, candidate geometry, cancel/commit, focus loss, stale-session rejection, and one-step undo pass; actual input methods/candidate placement remain unverified |
| F4: editable accessibility | PARTIAL; screen reader NOT RUN | Editable tree topology and selection/replacement actions pass synthetic tests; VoiceOver, stable run identity across edits, and offscreen navigation remain open |
| F5: host and clipboard lifecycle | PARTIAL | Headless save/snapshot fixture passes; native copy/paste interchange is unverified and destructive cut is deferred until clipboard-success acknowledgement has a defined path |

## Executed engine and host evidence

- Core package: 27 tests pass. The 480 generated cases include sequences of Unicode source edits, disjoint multi-changes, and random edit/undo/redo interleavings with stale submissions. Checks compare against independent String/history models rather than only roundtripping the implementation.
- Independent adversarial regressions cover invalid byte boundaries and resulting selections, unordered/adjacent changes, foreign sessions, stale revisions, save acknowledgement races, dirty-state restoration, divergent redo, BOM/mixed endings, bounded history, cross-chunk graphemes, and deletion from partial grapheme positions.
- Review identified an interior-grapheme deletion bug. It was fixed; the associated combining-mark/partial-selection regressions pass.
- Generic host: two integration tests and its executable pass. The executable reports `revision=6, committed_events=6, bytes=32, clean=true` while preserving the opening snapshot and save/history behavior.
- Native dependency boundary script passes on `aarch64-apple-darwin`. Core runtime dependencies are Ropey, its storage helpers, and unicode-segmentation. Markdown/layout boundaries transitively use core and do not import UI/application crates.
- Complete workspace: **38 tests pass** — 27 core, nine native adapter, and two generic host tests. All other crate test/doctest targets also complete without failure.
- Workspace formatting check, strict clippy for all targets, warning-free rustdoc, shell syntax checks, and the boundary script pass locally. The CI matrix is configured but has not run on a remote service or on other OS/toolchain combinations.

## Native adapter and visible runtime evidence

The nine adapter tests use our original controller and egui's input/paint/semantic APIs, not an existing editor widget. They exercise provisional composition and one history step; stale commit rejection; focused native event routing and candidate output; window-focus loss; dismissed preedit followed by document replacement; same-revision selection movement after preedit dismissal; one-event duplicate suppression; legal-grapheme geometry; and editable accessibility selection/replacement with undo. Tree assertions verify that runs have a single parent and that character lengths match source text.

Review exposed focus-loss cancellation, discarded composition ownership/selection anchors, duplicate-event, stale-geometry, and collapsed-selection paint issues. These were corrected before final workspace validation. The final IME guard retains both snapshot and selection, rejecting a commit after selection movement without changing source, revision, selection, or history. The headless harness explicitly releases toolkit texture deltas because it has no GPU consumer; its behavior assertions remain enabled.

The final debug demo was launched on the visible macOS desktop with the OpenGL backend and a local system CJK fallback font:

```sh
bash scripts/run-demo.sh --smoke \
  --screenshot /absolute/project/path/docs/evidence/native-feasibility.png \
  --font '/System/Library/Fonts/Hiragino Sans GB.ttc'
```

The run completed with `SCREENSHOT_SAVED` and `NATIVE_SMOKE ... rows=17 screenshot=true os_ime=NOT_RUN screen_reader=NOT_RUN`. The 2000×1600 [captured native frame](evidence/native-feasibility.png) was visually inspected. It shows the original Source surface, caret, controls, and unmodified sample Markdown. Latin and CJK glyphs appear, while Indic and Arabic/Hebrew samples have missing glyphs and the family emoji is not a composed ZWJ glyph. This is useful failure evidence for F2, not international-text acceptance. No alternate backend has been accepted.

Final demo binary SHA-256: `e649fc78d66df36271807aa7f7a02e7924cce660486fb616bdb899373196add5`.

Captured frame SHA-256: `35d6c3e4d20ae91ac3a4412865389fa97f6167354fb0c73f7967feac5eab5a14`.

The native dependency tree and direct linked-library inspection show platform graphics/window/accessibility libraries and no direct WebKit, JavaScriptCore, Node, or V8 link. This is the standalone native demo's evidence; it is not a claim that an arbitrary embedding host has no WebView. Symbol searches alone were not used as the acceptance test. No packaged or release build/performance qualification is claimed.

The `--smoke` process can time out with exit code zero even if a screenshot fails. Accept rendering evidence only when the screenshot marker, `screenshot=true`, and inspected file all agree, as they do for this run. The smoke mode does not type through real OS input methods or exercise a real screen reader.

## Reproducible automated checks

```sh
cargo fmt --all -- --check
cargo test --locked --workspace
cargo run --locked -p glyphfolio-editor-host-example
cargo clippy --locked --workspace --all-targets -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --locked --workspace --no-deps
bash scripts/check-boundaries.sh
```

The initial shell may need its installed Rust toolchain's `bin` directory on PATH. This workspace does not modify another project's environment scripts or toolchain settings.

## Required real-platform procedures

Run the native demo on a visible local desktop and record the exact build, backend, OS, fonts, scale, and keyboard/input method. Preserve failures as failures.

1. Inspect Latin ligatures, combining sequences, CJK, Indic text, emoji/ZWJ, and mixed Hebrew/Arabic/Latin. Verify click placement, logical selection, visual navigation, fallback, wrapping, and scaling. Record unsupported bidi/cluster behavior before selecting the production backend.
2. With real Japanese and Chinese input methods, replace a selection; update/cancel/commit preedit; undo one composition; scroll; switch scale/monitor; transfer focus; request a mode switch and close while composing. Confirm canonical text excludes provisional input and the candidate window follows the painted caret.
3. With VoiceOver on macOS, read, focus, navigate, select, replace, undo, and reach text initially outside the viewport. A semantic tree and synthetic accessibility action test are insufficient. Repeat with NVDA/Windows and Orca/supported Linux sessions before claiming those platforms.
4. Copy/cut/paste canonical Markdown to and from another application. Verify cut failure behavior and selection boundaries. Formatted HTML paste is outside this increment.
5. Exercise width/font changes, large notes, long single blocks, repeated view replacement, and resource retention only after the relevant layout/virtualization work exists. No 60-fps or 256-MiB acceptance is claimed by this increment.

## Known scope limits

The Markdown/projection and independent layout crates are Phase 0 boundaries. Rich rendering, CommonMark projection, incremental parsing, virtualization, formatted clipboard import, assets/tables, and GlyphFolio adoption are not implemented or qualified by the initial increment.

The first engine prioritizes correctness and intentionally retains some full-document operations, including explicit export and initial navigation/comparison paths. Their performance must be profiled before optimization or large-document acceptance.

The native adapter lays out one small Source galley and uses visual-row accessibility IDs. Production block layout, stable semantic identity across edits/reflow, offscreen accessibility scroll actions, complete native shortcut behavior, and a clipboard acknowledgement contract remain future work. Rich mode is explicitly labeled as a Source fallback. This increment completes the initial engine/scaffold work and starts executable native feasibility; it does not complete M1 or pass the full F2–F5 gates.

The prototype conservatively rejects commit-only IME events after explicit cancellation until a new nonempty preedit starts a new input session. This prevents late commits from mutating another selection/document but can reject legitimate commit-only input such as some emoji-picker paths. It is an open F3 input-epoch limitation, not accepted international input behavior.
