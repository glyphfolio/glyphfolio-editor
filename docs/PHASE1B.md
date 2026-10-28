# Phase 1B native feasibility record

Historical report: its Rust 1.95 declaration is superseded. The preview now requires Rust 1.99.1, matching the locally tested toolchain; no older-toolchain compatibility is claimed.

Historical checkpoint. The later autonomous implementation and current results are recorded in [Phases 1–6](PHASES_1_6.md). Earlier binary identities and failures below are retained as history. Shared evidence filenames may contain later reruns; use the current report for their exact build and result.

This record continues the [initial increment](VALIDATION.md), whose older screenshot and failed shaping configuration remain historical evidence. Rich editing has not started. Work is confined to this independent workspace; the existing GlyphFolio application is unchanged.

## Implemented boundaries

- Original source geometry and navigation over low-level COSMIC Text shaping, font discovery, and Swash rasterization. The engine does not use COSMIC Text's editor, history, selection, hit-testing, or editing controller. Core, Markdown, and layout remain egui-free.
- Font fallback, color emoji textures, bidirectional visual navigation, legal grapheme caret stops, wrapped selection geometry, and scale-aware rasterization. System fonts are discovered locally and are not redistributed.
- Input epochs bound to document identity, revision, and selection; provisional composition stays outside canonical Markdown. Composition commits are atomic; interruptions invalidate old epochs. Empty preedit retains ownership until an explicit cancel, commit, or interruption. Toolkit events without native provenance still require conservative routing.
- An editable AccessKit tree with stable paragraph/run identity, UTF-16 platform selection conversion, source replacement, and offscreen scroll actions. Native action delivery and actual screen-reader behavior are separate acceptance requirements.
- A host-owned synchronous clipboard acknowledgement boundary. Cut deletes only after successful publication and revision/selection validation. Clipboard errors and stale captures leave source and history unchanged.

## Native test safeguards

The user authorized temporary Accessibility, VoiceOver, and Japanese/Chinese input-method setup. Tests target only this workspace's disposable demo and synthetic text. Native scan-code helpers verify the exact executable, process identity, focused text area, and source fixture before posting input; they abort on lost focus. These are programmatic native events, not physical keyboard testing.

The input-source driver journals temporary enabling, restores the prior selected source, and verifies the original enabled configuration and source flags. Clipboard probes retain all original representations in memory and restore their exact bytes; they do not log clipboard payloads. A new clipboard owner prevents restoration from overwriting new user content. Accessibility helpers bound native queries and preserve failures.

`tools/native-app/Info.plist` describes the local, ad-hoc-signed test bundle. It is not a distribution package or a notarized release.

## Evidence and qualification

**Checkpoint: Phase 1B remains open. Repeated desktop test iterations have stopped. Rich editing is gated on the unresolved native checks below.** A synthetic test, successful setup toggle, or AccessKit tree does not pass a real screen-reader gate.

| Gate | Result | Evidence and remaining boundary |
| --- | --- | --- |
| F2: shaping/geometry | PASS for tested local Source coverage | Seven layout tests and an inspected native frame cover Latin ligatures, combining marks, CJK, Indic conjuncts, Arabic/Hebrew, emoji/ZWJ, fallback, wrapping, bidi geometry, and scale-aware images. Font discovery found 807 faces; the sample had zero missing glyphs/raster failures and two color glyphs. This is not every script/font or a production layout qualification. |
| F3: native IME | PARTIAL | Recorded real Japanese and Chinese commits replace TARGET with 日本/中文, keep preedit out of canonical source, and undo in one step. Native cancellation failed: one Escape left composition pending. A revised explicit cancellation sequence and native viewport/focus exercises are prepared but NOT RUN after their final changes. |
| F4: editable accessibility | PARTIAL | Native AX text exposure, Unicode/UTF-16 selection, replacement, undo, idle actions, and offscreen scrolling passed. Actual VoiceOver reading/navigation/editing did not pass. Starting the service and restoring settings have been proven separately; they do not qualify the editor. |
| F5: clipboard/host | PARTIAL | Cross-process clipboard transport preserves exact UTF-8/CRLF. Native widget copy and acknowledged cut passed, but paste failed. Its omitted toolkit feature is corrected and CRLF recovery has a passing regression; the fixed native path and extended Unicode/CRLF test still need one runtime retest. |

The final source passes **78 tests**: 27 core, 25 adapter/accessibility/clipboard, 15 composition-session, seven layout, two generic-host, and two demo-probe tests. Formatting, strict workspace Clippy, warning-free rustdoc, and native dependency boundaries also pass. Validation used Rust 1.99.1 on macOS 26.6.2 arm64; the declared 1.95 floor and other OSs are configured in CI but were not executed here.

The selected candidates are COSMIC Text 0.19.0, egui/eframe 0.37.0, AccessKit 0.25.0, and arboard 3.7.0. The engine, Markdown, and layout dependency graphs remain free of egui, Tauri, WebView, and GlyphFolio application dependencies.

The initial native runs exposed two distinct failures: some accessibility actions were processed only after another OS key event, and some Japanese test runs timed out after reaching the native conversion path. Failed runs and source-recovery evidence are retained in `evidence/platform-ax*.json`. Native helper focus/discovery races are distinguished from editor behavior. Neither timeout nor successful key posting is composition acceptance.

The toolkit upgrade includes pending-redraw deadline handling in the [upstream native event loop](https://raw.githubusercontent.com/emilk/egui/0.37.0/crates/eframe/src/native/run.rs); the later native AX run passed idle updates and scrolling. Attribution to that change alone is an inference, since other adapter corrections were present. No dependency source was patched and no continuous repaint workaround was added to the editor.

Native testing exposed an upgrade integration error: eframe 0.37 makes native clipboard support an explicit feature. Enabling it fixes the identified missing read path. The toolkit also normalizes CRLF paste text; the adapter now recovers the host clipboard's raw bytes only when they match that captured payload after normalization. An unrelated/programmatic payload retains its supplied text.

Original adapter fixes include correct selection collapse, focus locks for arrows/Escape, bounded duplicate suppression, full Indic-conjunct end geometry, and paragraph-level word boundaries that survive soft wraps. The native helpers now compare AX elements for equality rather than treating hash values as unique; the earlier hash-only traversal skipped some setup controls.

### Recorded artifacts

- [Multilingual Source preview](evidence/phase1b-shaping.png), SHA-256 `618338f2096a1d552e1480846ed76376c9e7516452ac05b6c1fe82f5a84889b8`.
- [Japanese commit](evidence/platform-japanese.log) and [Chinese commit](evidence/platform-chinese.log): actual native preedit/commit events and one-step undo, using guarded programmatic scan codes through the OS input method.
- [Cancellation failure](evidence/platform-ime-cancel.log), [native AX results](evidence/platform-ax.json), and [clipboard failure/restoration](evidence/platform-clipboard.json). These remain failure/partial evidence; prepared fixes do not rewrite them into passes.
- [Japanese](evidence/platform-japanese-candidate.json) and [Chinese](evidence/platform-chinese-candidate.json) scoped popup/caret bounds. Geometry places the controlled native popups beneath the caret; identification from timing/bounds is an inference. No window titles or screen pixels were read.
- [Final VoiceOver restoration](evidence/platform-voiceover-final-restoration.json) and [input-source restoration](evidence/platform-input-source-restoration.json).

Those desktop runs predate the last clipboard/word-boundary fixes. The signed binary used for the most recent native batch was `9bd10a0ff6be98c65107521c8f1e5de8cc7674eea8b91b093bcfd459a92dce6a`, retained locally under `target/evidence-before-final-fixes/`. The current unsigned debug binary is `5d62e6da31450c22e1994619002727a2deeb3af7c6898f7e047248d5fada822f`; its ad-hoc-signed review bundle is `bfd9fff91197ac548eda1f5090d969157f925c73cfdc97ff0060995ae5eefe3b`. **The final build has not been requalified through the desktop gates.** Signing changes the binary hash.

### Restored native state

The clipboard probe verified restoration of all prior items, types, and bytes. US input is selected and the original three input-source preference entries and disabled SCIM/Pinyin flags were independently restored. macOS retained a default-mode entry after both TIS disabling and normal UI removal; narrowly guarded public CFPreferences cleanup removed only that journaled temporary entry. Unrelated concurrent configuration changes prevent that cleanup from overwriting them.

VoiceOver is off, no VoiceOver engine remains, and its General AppleScript-control option is restored to false. The pre-existing welcome-reminder option is unchanged. Owned setup windows were closed. The Accessibility permission the user manually enabled remains enabled; no TCC database or permission reset was performed.

### Next bounded qualification

1. Retest the fixed widget clipboard path once, including exact Unicode Markdown/CRLF and discrete undo. Preserve the failure if it still fails.
2. Run the prepared native cancellation, viewport/scale, focus-transfer, and host-barrier checks once. A posted key or unchanged source without actual preedit cannot pass cancellation. Unexpected early probe exit is now an error.
3. Qualify actual VoiceOver reads, navigation, selection, replacement, undo, and offscreen text against the final build. Service flags and successful setup alone are insufficient.

The visible macOS helpers are experimental, opt-in, and require a supervised native session. `tools/run-native-gates.sh --allow-temporary-setup` does not run VoiceOver unless `--with-voiceover` is additionally supplied; a default batch cannot qualify F4. No further native test session is scheduled or running.

## Remaining scope

This increment does not qualify large-document latency, memory limits, virtualization, CommonMark projections, Rich editing, or GlyphFolio integration. Windows/NVDA and Linux/Orca are separate platform gates. Font fallback results depend on installed fonts. Ligature caret advances are interpolated rather than obtained from font GDEF caret tables.

The Source prototype currently uses a system sans-serif/fallback stack; its `FontId` setting controls size, not arbitrary families. Commit-only emoji ownership is covered by controller tests; the actual OS emoji picker and monitor-to-monitor composition are not qualified. The underlying toolkit does not tag native commits with our epochs, so explicit host interruption and stale-batch quarantine remain required.
