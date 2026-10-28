# Dependency and provenance policy

GlyphFolio owns the original editor architecture and behavior. No existing editor implementation may be adopted, wrapped, forked, extracted, copied, adapted, or ported into this project.

This excludes existing engine/controller components including Leaf, Ferrite, CodeMirror, Markion, Velotype, egui_richedit, and egui_pop_editors. Observable behavior may inform regression cases; existing editor source is not implementation material.

Permitted low-level infrastructure includes Ropey text storage, a CommonMark parser, Unicode algorithms, font discovery/shaping/rasterization, graphics, native window events, clipboard, and accessibility APIs. Review actual API use: permission to use COSMIC Text, Parley, or egui font APIs is not permission to use a ready-made editor controller, selection/history engine, or TextEdit widget as this editor.

Engine, Markdown, and layout crates must not depend on egui/eframe, Tauri, a WebView, the GlyphFolio application, its database, or its plugins. The native adapter is a separate crate. Run `bash scripts/check-boundaries.sh` and review new dependencies and borrowed assets before merging.

Preserve upstream notices and license terms. Do not redistribute system fonts; runtime font discovery or explicit local font paths are host responsibilities. Lock executable/demo dependencies and record exact build/toolkit versions in validation. Public API examples and tests use only the public library surface.
