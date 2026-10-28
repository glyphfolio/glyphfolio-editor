# Phases 1–6 implementation and qualification

Historical report: its Rust 1.95 declaration is superseded. The preview now requires Rust 1.99.1, matching the locally tested toolchain; no older-toolchain compatibility is claimed.

Date: 2026-10-28. For the later visual/configuration increment, see [Editor design](EDITOR_DESIGN.md). The binary identities and native-gate evidence in this report remain the original checkpoint. This supersedes the earlier incremental status in `PHASE1B.md` and `VALIDATION.md`; their older failures and binary identities remain historical evidence.

The user authorized autonomous implementation through Phase 6, skipping work that requires a password, permission prompt, or further interaction. All project edits are confined to this independent workspace. The current GlyphFolio application is unchanged. No existing editor engine/controller was adopted, copied, wrapped, or ported.

## Implementation delivered

| Phase | Current implementation | Qualification boundary |
| --- | --- | --- |
| 1A | Rope document, immutable snapshots, revision/session-safe atomic changes, maps, bounded history, observers, exact save acknowledgements and grapheme commands | Headless independent-model/property and adversarial tests |
| 1B | COSMIC Text shaping, original bidi/grapheme geometry, owned IME epochs, candidate output, native clipboard and editable AccessKit transport | Local font coverage and native input evidence; actual VoiceOver remains unverified |
| 2 | Source pointer/keyboard selection, double/triple click, drag scrolling, word/visual/page navigation, shortcuts, clipboard, native host and atomic save/reopen example | Native macOS transport plus event-route tests; other OSs unqualified |
| 3 | CommonMark reference parser, Copy/Hidden/Transform projections, nested styles, original byte offsets, source fallbacks and stable block identities | All 652 official CommonMark 0.31.2 examples and projection/edit/undo differential tests |
| 4 | Explicitly experimental Rich: active block reveal, heading typography, strong/emphasis/code, links, quotes, lists, fences; shared source selection/history | Synthetic integration tests and visible native preview; incomplete native gates prevent qualified-release claims |
| 5 | Conservative paragraph incrementality, indexed styles, stable layout blocks, Fenwick height index, source scroll anchors, visible/caret shaping and bounded derived caches | Measured headless workloads; no 60-fps or 256-MiB application claim |
| 6 | Structural commands and toolbar, native demo, generic public host, exact atomic file save/reopen, heading semantics, platform qualification harness | Local tests and qualified native subchecks; screen-reader and other-platform acceptance remain open |

Source is the default. Rich requires `enable_experimental_rich(true)` and `request_presentation(PresentationMode::Rich)`. This opt-in makes unfinished work reviewable while preserving the approved native gates. It does not declare Phase 1B or M1 qualified.

## Correctness and interaction

All changes use original core transactions and the same history. Source/Rich switches change derived presentation, never serialize an AST back to Markdown. Projection maps preserve entities, escapes, normalized code spans and hidden syntax. Partial transformed selections require source reveal rather than silently guessing a source range. Accessibility selections use inward, direction-aware endpoints; collapsed selections stay collapsed at hidden delimiters.

The original source commands support strong/emphasis/inline code, H1–H6, ordered/unordered lists, quotes, fenced code, indent/outdent, list/quote continuation, empty-item exit and structural backspace. They preserve local endings and BOM bytes and undo atomically. These are deliberately source-line commands, not a general schema-driven rich-text transformation framework.

IME preedit remains disposable and cannot appear in save/undo history. A current document identity, revision, selection and input epoch authorize commit. Toolbar formatting refuses active composition. Focus boundaries invalidate old input; a queued mode/close request waits until composition ends. During Rich composition the surface temporarily reveals Source, then returns to the requested Rich view. Commit-only input and duplicate native Text events have explicit guards.

The generic host example writes actual UTF-8 chunks to a same-directory temporary file, syncs and atomically replaces the requested path, then acknowledges that exact snapshot. A stale completion cannot mark newer edits clean. Reopening creates a new document session. The demo accepts an explicit save path; no file-picker or permission dialog is required.

## Performance evidence and limits

The parser benchmark uses 10,000 paragraphs / 288,890 bytes. Thirty local release samples measured full parse p95 2.995 ms, projection p95 5.699 ms, and conservative incremental update p95 1.220 ms. Grammar incrementality currently accepts only safe unstyled ASCII paragraph changes; structural, reference, container and Unicode edits fall back to the full correctness path. Source materialization and suffix shifts remain O(N).

The layout benchmark uses 10,001 Source lines / 318,890 bytes. One local release run measured first viewport 24 blocks / 19.860 ms, warm unchanged passes mean 0.008 ms with zero reshaping, and one middle-line update 5.172 ms including reconciliation. Renderer tests separately cover 10,001 Source lines and a 10,000-line Rich fence, each shaping fewer than 40 blocks for the tested viewport. Remote caret neighborhoods are resolved explicitly.

Default retained layout accounting is 128 blocks / 32 MiB; raster retention is bounded separately. These bounds exclude font-library internals, GPU allocations, host-held Arcs and temporary buffers. Metadata reconciliation, projection construction and compatibility geometry still scan/copy full strings on invalidation. A Rich paragraph over 32 KiB with more than 512 style intervals now explicitly displays Source before attributed shaping. The demo reports this safeguard and the host can inspect `large_rich_source_fallback` / `rendered_presentation()`. This avoids a measured backend attribution bottleneck; it does not qualify giant Rich paragraphs. An enormous unbroken Source line uses an explicit exact shaping fallback and can block; the independent layout API also offers an explicit deferred policy. The measured 100,000-byte exact paragraph took 48.886 ms. Rich code fences are split at actual hard lines, without truncating source.

When accessibility activates, complete projected geometry is prepared so offscreen editable text exists. An unchanged Arc and semantic row index avoid repeated full shaping/indexing on selection-only updates. This is a functional fallback with whole-document cost; accessible large-document latency/memory is not qualified. A synthetic 10,001-row semantic fixture needed 40,001 indexed lookups / 80,002 binary probes; this is algorithmic work evidence, not screen-reader or rendering performance.

See the layout and Markdown crate READMEs for reproducible benchmark commands and exclusions.

## Native gates and skipped work

Executed run results and exact build identities are recorded below. Native AX transport is distinct from VoiceOver interaction. A tree, successful service launch or synthetic action cannot pass F4.

- VoiceOver reading, navigating, selecting, replacing, undoing and offscreen traversal: **SKIPPED**. Completing setup currently requires interactive welcome/control prompts. The user requested prompt-bound work be skipped; no setup request was issued overnight.
- Windows/NVDA and Linux/Orca runtime qualification: **NOT RUN** on this macOS host. The CI build/test matrix is configured; it has not run remotely. Rust 1.95 is declared but only local Rust 1.99.1 is installed/tested.
- Formatted HTML paste, tables, assets, collaboration, LSP and existing-editor plugins remain outside M1.
- Real-user usability, all script/font coverage, long-duration soak/fuzz campaigns, total process memory and sustained frame deadlines remain separate acceptance work.

## Final local evidence

All **165 workspace tests** pass, plus the documentation test targets. [Structured qualification summary](evidence/phases1-6-qualification.json) and [automated run log](evidence/phases1-6-automated.log) retain the results. Formatting, strict workspace Clippy/all targets, warning-free rustdoc, shell syntax and native dependency checks pass. The generic host executable reports an actual `exact_utf8=true` save/reopen roundtrip. Local environment: macOS 26.6.2 (25G83), Apple Silicon, Rust 1.99.1, egui/eframe 0.37.0, AccessKit 0.25.0 and COSMIC Text 0.19.0. The selected native binary has no direct WebKit/JavaScriptCore/Node/V8 linkage.

| Native check | Result | Evidence and scope |
| --- | --- | --- |
| Japanese commit, replacement and undo | PASS in controlled Source fixture | [Final viewport run](evidence/overnight-ime-viewport.log): real preedit/commit, original source unchanged provisionally, one-step undo |
| Native scrolling, scale and host barriers | PASS in same run | Scroll and zoom observed during composition; requested mode/close stayed deferred; source and native input settings restored |
| Japanese cancel | PASS using explicit editor Cancel command | [Checkpoint batch](evidence/overnight-native-batch.log); three Escape presses followed by the owned editor command; Escape alone is not qualified |
| Chinese commit and focus transfer | PASS in checkpoint Source fixtures | Same checkpoint batch: actual 中文 commit and undo; Japanese own-app focus loss cancels without persisting preedit |
| Candidate placement | Scoped geometric evidence | Controlled native popup bounds lie near the reported caret; identification uses timing/bounds, not captured popup text/pixels |
| Editable native AX transport | PASS for executed actions; F4 remains PARTIAL | [Final AX report](evidence/platform-overnight-final-ax.json): text, UTF-16 selection, replacement, undo and offscreen action; actual VoiceOver NOT RUN |
| Native clipboard | Prior checkpoint PASS; final repeat SKIPPED safely | [Prior successful run](evidence/overnight-clipboard-prior-build.json) covers native copy/cut/paste, Unicode/CRLF and undo with all prior representations restored. [Later attempt](evidence/platform-clipboard.json) could not preserve a current clipboard representation and stopped before mutation. This is not a final-build clipboard requalification |
| Native rendering and large documents | PASS for the five bounded smoke cases | [Machine-readable results](evidence/native-smoke-matrix.json); each ran 64 native callbacks, saved a screenshot and exited normally. Source/Rich and large-document images were inspected |

Final unsigned release binary SHA-256: `adb09c69189bfde1be0fff27fff6de6913fb41b9a5dd5b9e2a07e44e005e696a`. Ad-hoc-signed local review bundle executable: `fb09c711bd079772d94435d3d019dcee6e90c07624cc60a21d2ccafe9231d0c3`. These identify the final preview/smoke, Japanese viewport and final AX runs. Earlier cancellation/Chinese/focus checkpoint binary: unsigned `5cf85d8ddf0f2d85849fadcaa10e5508140e57cbac45643a7952b27f9e053b26`, signed `fe67b013d4867eaf19d7a6fc9d98c0c8df34f48ebf833150c740204b29bcc33d`. The earlier fully passing clipboard run used signed `bfd9fff91197ac548eda1f5090d969157f925c73cfdc97ff0060995ae5eefe3b`.

### Native callback and memory observations

| Final native workload | Cold UI callback | Warm UI callback p95 (61 samples) | Maximum sampled process RSS |
| --- | --- | --- | --- |
| Small Source | 63.3 ms | 0.283 ms | 211.2 MiB |
| Small Rich | 63.9 ms | 0.292 ms | 209.4 MiB |
| 10,000 Source hard lines | 40.9 ms | 0.440 ms | 224.9 MiB |
| 10,000 Rich paragraphs / 20,000 blocks including blanks | 98.0 ms | 1.130 ms | 235.3 MiB |
| Pathological styled paragraph, explicit Source safeguard | 46.5 ms | 0.508 ms | 227.9 MiB |

These callback timings exclude GPU completion; RSS samples come from the owned demo process approximately every 100 ms and may miss peaks. They do not establish sustained 60-fps behavior or GlyphFolio's whole-application 256-MiB target. No accessibility/screen-reader was active in these five smoke cases.

The [pre-safeguard giant-paragraph run](evidence/m1-large-rich-before-paint-fix.log) timed out after 25 seconds. Investigation found quadratic style orchestration, unbounded paint submissions and a backend attributed-word scan. Original orchestration/paint loops are corrected and covered by resource counters; the remaining backend case has the visible Source safeguard, not a fabricated Rich pass. The 10,000-line/20,000-font-run shaper regression now visits exactly 20,000 runs. The 10,000-style paint regression uses tiled row geometry to isolate viewport query behavior from the unqualified backend case.

[Source preview](evidence/m1-source.png), [Rich preview](evidence/m1-rich.png), [10,000 Rich paragraphs](evidence/m1-large-rich-paragraphs.png), [large-paragraph safeguard](evidence/m1-large-rich.png).

US input and the exact pre-test input-source configuration were independently verified restored after the final native input run. VoiceOver remained off and no setup prompt was invoked. The clipboard preservation failure made no clipboard change. The existing manually granted Accessibility permission remains enabled. No test window/process or recurring test job is intentionally left running.

## Reproduction

```sh
cargo test --locked --workspace --all-targets
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --locked --workspace --no-deps
bash scripts/check-boundaries.sh
cargo run --locked -p glyphfolio-editor-host-example -- --save-reopen /explicit/existing/folder/roundtrip.md
cargo run --locked -p glyphfolio-editor-demo
cargo run --locked -p glyphfolio-editor-demo -- --experimental-rich
cargo run --locked -p glyphfolio-editor-demo -- --generate-lines 10000 --experimental-rich
```

`--source-file PATH.md` reads exact UTF-8; `--save-path PATH.md` enables a host-owned atomic save button. Those paths are explicit host decisions; external file conflict handling remains the host's responsibility.

The separately attributed official CommonMark test corpus has its own CC BY-SA 4.0 terms in `crates/editor-markdown/tests/fixtures/COMMONMARK-LICENSE.md`. Original project source remains MIT OR Apache-2.0. Publishing is disabled pending API/release review.
