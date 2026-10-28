# Independent layout and viewport shaping

This original crate supplies source/display geometry, bidi caret movement, font runs, block flow, a Fenwick height index, content anchors, and bounded visible shaping. COSMIC Text supplies low-level shaping, wrapping, font discovery, and rasterization; its editor/controller is not used.

`SourceDocumentBlocks` reconciles source hard lines and preserves stable line IDs through prefix edits and single-line replacements. `LayoutBlock` can instead describe Markdown projection leaves using one shared `Arc<str>`, source/display ranges, typography, and disjoint local font spans. `VirtualDocument::set_blocks` validates atomically and preserves unchanged measured heights/cache entries. The UI adapter owns source-map conversion and block adornment painting.

Use `layout_viewport` for visible/overscan work. Its measured blocks carry global bounds, local shaped documents, and text origins. Its caret/hit/selection methods return display coordinates. Resolve a remote caret with `ensure_display_byte` or `layout_around_display` before navigating; missing geometry must not be replaced with the nearest visible caret. Capture/resolve block scroll anchors around source or typography changes. Source anchors must be mapped through the host's committed change map when their blocks are removed.

Width, scale, font/typography generations, block content generations, actual text, and font spans participate in layout invalidation. Font-file loads and replacement shaping engines invalidate the virtual cache automatically. Default derived layout retention is 128 blocks / 32 MiB, and visible passes consider at most 128 blocks. Owned raster retention is bounded separately to 4096 images / 32 MiB RGBA capacities and 8192 miss queries between resets. These accounting bounds exclude font-library internals, GPU textures, and temporary/host-held image Arcs; they are not a process-memory claim.

The default long-block policy defers blocks over 32 KiB and exposes an unmeasured placeholder. `LongBlockPolicy::Exact` is an explicit functional fallback and reports that it performed unbounded whole-block work. Source hard-line blocks handle long multiline files. An enormous unbroken paragraph still requires exact work or deferred/background resolution; whole-block virtualization does not make it cheap. Rich fenced blocks should be split at exact hard-line boundaries by the adapter while preserving their projection/source map and parent styling.

`ViewportLayout::to_shaped_document` is a compatibility bridge for adapters still using a full display string. It globalizes measured rows/glyphs but scans/retains the complete text. Its use, whole-document accessibility fallback, source reconciliation, and source-string materialization remain separate costs.

## Reproducible headless measurement

```sh
cargo run --locked --release -p glyphfolio-editor-layout --example virtual_benchmark
```

One macOS arm64 / Rust 1.99.1 release run with 807 discovered font faces measured 10,001 source-line blocks / 318,890 bytes:

| Operation | Observation |
| --- | --- |
| Metadata construction | 5.641 ms; full source scan |
| Font-system initialization | 226.059 ms; excluded from layout measurements |
| First viewport | 24 shaped blocks / 566 glyphs, 19.860 ms |
| Twenty unchanged viewport passes | 0 reshaped blocks, mean 0.008 ms |
| Full-document reference shape | 258,890 glyphs, 336.662 ms |
| Twenty remote scroll samples | 502 shaped blocks total |
| Maximum retained layout cache | 128 entries / 1,730,152 accounted bytes |
| One middle-line edit | One reshaped block, 5.172 ms including source reconciliation |
| 100,000-byte paragraph, deferred | Zero shaped blocks, 0.001 ms frame work |
| Same paragraph, explicit exact fallback | One whole block, 48.886 ms |

These are one-run headless work/latency observations, not 60-fps, GPU, native input, accessibility, total-memory, or multi-platform qualification. The compiler emitted a local `rust-objcopy` missing-libLLVM warning while attempting release debug-info stripping; compilation and the executable completed successfully.
