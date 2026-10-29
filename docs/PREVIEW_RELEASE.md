# v0.1.0 preview release preparation

Status: public source preview at [glyphfolio/glyphfolio-editor](https://github.com/glyphfolio/glyphfolio-editor). Private security reporting is enabled. The initial source commit passed the six-job CI matrix; no crates.io upload or installer distribution is planned.

## Supported claims

This source preview provides an independent embeddable editor with an original document/editing engine and an egui adapter. The demo demonstrates rich Markdown and native table editing. APIs can change before 1.0. Existing GlyphFolio applications are outside this repository and unchanged.

Local checks cover macOS arm64 with the installed Rust toolchain. Cargo requires Rust 1.99.1, matching the locally tested version; older versions are not claimed as supported. The [initial CI run](https://github.com/glyphfolio/glyphfolio-editor/actions/runs/40791003700) passed builds and automated checks on stable and Rust 1.99.1 across macOS, Linux and Windows. Interactive Windows/Linux behavior remains unqualified. Full cross-target dependency metadata cannot be resolved offline on this machine because some target-only crates are not cached.

## Known limitations

- Real VoiceOver editing and the complete native IME/clipboard platform matrix remain unqualified. Synthetic input tests do not establish OS behavior.
- Rich composition temporarily uses Source presentation. Very large styled paragraphs retain a Source safeguard.
- Tables shape as whole blocks with equal/minimum column widths. Resizing, merged cells and rectangular spreadsheet selection are deferred. Unsupported/ragged/container tables retain source fallback.
- Asset resolution/import belongs to hosts; the demo is not GlyphFolio's attachment broker, database, sync or plugin integration.
- HTML paste is a bounded converter; malformed and application-specific clipboard formats need more coverage.
- This preview does not establish GlyphFolio's full-application memory target or remove its WebView.

## Local preparation checks

Run `bash scripts/check-preview.sh`. It runs formatting, workspace tests, strict Clippy, warning-free rustdoc, dependency-boundary checks and the public-file audit. Build the demo with `cargo build --locked --release -p glyphfolio-editor-demo`. The latest local record is [preview validation](evidence/preview/validation.json); native rendering evidence remains [separately scoped](evidence/rich-stable/validation.json).

The public-file audit checks common credential signatures, private absolute paths and accidental build/cache files. It is a bounded automated review, not a guarantee that no secret can exist. Historical evidence paths were redacted; no credentials were found by the preparation scan.

## Publication record

The public repository and private security reporting are configured. Initial source commit `1b68dfc0f4425366c529bd27df57218e39884386` passed the six-job CI matrix. The documentation-only follow-up records those results; its source code is identical to the tested commit. Historical local reports retain their original scope.

Future release candidates should run the same CI matrix on their code revision and retain explicit interactive-platform limits. Source preview release notes are in [CHANGELOG.md](../CHANGELOG.md).

All crates retain `publish = false`. No binaries, system fonts or dependency source are included in this source preview. If distributing binaries later, include the relevant upstream license texts/notices and review all target dependencies using their locked versions. [Dependency inventory](DEPENDENCY_LICENSES.md), [provenance policy](DEPENDENCY_POLICY.md), and [third-party notices](../THIRD_PARTY_NOTICES.md) explain scope.
