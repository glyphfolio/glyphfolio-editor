# Third-party material and notices

Original project implementation is licensed MIT OR Apache-2.0; see the root license files. Dependency and fixture licenses remain separate.

- `crates/editor-markdown/tests/fixtures/commonmark-0.31.2.json`: official CommonMark 0.31.2 examples by John MacFarlane, CC BY-SA 4.0. Source, attribution and license links are retained in [the fixture notice](crates/editor-markdown/tests/fixtures/COMMONMARK-LICENSE.md). This data is not relicensed under the Rust implementation license.
- Cargo dependencies are obtained from their upstream packages under their own licenses. [Locked native dependency inventory](docs/DEPENDENCY_LICENSES.md) records declared licenses; retain upstream notices when redistributing dependency code or binaries.
- System fonts are discovered at runtime or explicitly supplied by the host. No system font files are bundled.
- Toolbar icons are original Rust-painted geometry. The demo's `sample.png` is a synthetic gradient fixture; editor screenshots use synthetic demonstration content. No third-party logo or icon pack is bundled.

This repository does not include an existing editor engine implementation. See [dependency policy](docs/DEPENDENCY_POLICY.md).
