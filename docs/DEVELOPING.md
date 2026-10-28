# Developing glyphfolio-editor

This independent library workspace contains the editor and its demos. Work here does not modify or replace the existing GlyphFolio applications. Existing editor engines are neither dependencies nor source to port. See the [dependency and provenance policy](DEPENDENCY_POLICY.md).

## Platform setup

The workspace requires Rust 1.99.1, matching the locally tested toolchain. CI is configured for Rust 1.99.1 and stable on macOS, Windows and Linux; remote runs remain unexecuted. See [preview compatibility](PREVIEW_RELEASE.md) for the qualification scope.

- macOS: install Xcode Command Line Tools and the Rust toolchain.
- Windows: install the MSVC Rust toolchain and Visual Studio C++ build tools.
- Linux: install your distribution's native windowing development dependencies. The Ubuntu CI configuration uses:

  ```sh
  sudo apt-get update
  sudo apt-get install -y libxkbcommon-dev libwayland-dev libegl1-mesa-dev libxcb-shape0-dev libxcb-xfixes0-dev
  ```

Python 3 is used by the public-file audit. Ordinary editing does not require permission changes. The optional native probe tools can require platform permissions and should be run deliberately.

If Cargo is missing from PATH but Rust is installed in a standard rustup toolchain directory, `bash scripts/run-demo.sh` also locates the installed stable toolchain.

## Local checks

Run the complete preview gate:

```sh
bash scripts/check-preview.sh
```

It checks formatting, workspace tests, strict Clippy, warning-free rustdoc, dependency boundaries and public-source hygiene. Individual commands:

```sh
cargo test --locked --workspace
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --locked --workspace --no-deps
bash scripts/check-boundaries.sh
python3 scripts/check-public-files.py
```

For headless engine work and a generic save/reopen host:

```sh
cargo test --locked -p glyphfolio-editor-core -p glyphfolio-editor-host-example
cargo run --locked -p glyphfolio-editor-host-example
```

## Demo diagnostics

The low-level adapter defaults to Source; the standalone profile opts into Rich through `editor.toml`. The standalone retains its diagnostic surface behind `--native-test-ui`; IME probe modes select it automatically.

```sh
cargo run --locked -p glyphfolio-editor-demo -- --generate-lines 10000 --experimental-rich
cargo build --locked --release -p glyphfolio-editor-demo
```

Pass `--smoke --screenshot /absolute/path.png` for a short visible rendering check and capture. `--font /path/to/font.ttf-or-ttc` loads a host-supplied font without bundling it. Rendering captures are not OS input or screen-reader qualification.

The selected native runtime does not require a browser, WebView, JavaScript or Node. Cargo's lockfile can still contain toolkit dependencies for other target platforms; boundary checks inspect the selected native target.

## Validation and implementation history

The recorded local workspace result is **230 passing tests**, including the 652 official CommonMark examples inside one parser corpus test. Formatting, strict Clippy, rustdoc and native dependency checks passed on macOS arm64 with Rust 1.99.1. These are recorded results; rerun the gate against a new revision before making fresh claims.

- [Current preview preparation and limitations](PREVIEW_RELEASE.md)
- [Machine-readable preview results](evidence/preview/validation.json)
- [Rich editing and native table evidence](evidence/rich-stable/validation.json)
- [Earlier editing-parity increment](EDITING_PARITY.md)
- [Historical Phases 1–6 qualification](PHASES_1_6.md)
- [Historical Phase 1B checkpoint](PHASE1B.md)
- [Visual design history](EDITOR_DESIGN.md)

Actual VoiceOver editing, the complete native input/clipboard platform matrix and Windows/Linux runtime behavior remain unqualified. Large styled paragraphs and active Rich composition retain Source safeguards. The [preview notes](PREVIEW_RELEASE.md) keep the detailed limitations together.

## Architecture and release references

- [Implementation plan](../IMPLEMENTATION_PLAN.md)
- [Public API fixture](PUBLIC_API_FIXTURE.md)
- [egui adapter](../crates/editor-egui/README.md)
- [Layout behavior and measurements](../crates/editor-layout/README.md)
- [Markdown projection](../crates/editor-markdown/README.md)
- [Dependency license inventory](DEPENDENCY_LICENSES.md)

Public APIs are pre-1.0. Crates retain `publish = false`; publication requires separate approval. No publication is performed by the development checks.
