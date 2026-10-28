# Public embedding design fixture

This fixture was specified before the initial engine implementation. Its purpose is to make the API useful to an ordinary Rust host without GlyphFolio, egui, parser ASTs, or rope internals.

The executable and integration tests under `examples/host-integration` must demonstrate:

1. Create a document from exact UTF-8 Markdown; retain an immutable opening snapshot.
2. Subscribe to committed change notifications without receiving presentation-only events.
3. Apply an atomic transaction based on that snapshot and observe its current session/revision.
4. Keep Source and Rich view preferences separate from canonical bytes and shared history. A Rich preference in this fixture does not imply Rich rendering exists.
5. Retain a save snapshot, make a newer edit, and acknowledge the older save without marking the newer state clean.
6. Acknowledge the current saved snapshot and become clean; reject another document's acknowledgement.
7. Undo/redo against current monotonic revisions, preserving original source bytes and selection behavior.
8. Refuse a stale transaction and malformed byte range atomically; the host can recover using a fresh snapshot.

No host should need direct access to a Ropey rope, a parser AST, an egui context, or layout caches for these operations. Full text export must be explicit. Save acknowledgement identifies a document and snapshot, not just a global numeric counter.

The fixture now compiles against the Phase 1A engine. Run `cargo run --locked -p glyphfolio-editor-host-example` and its public-API integration tests. The headless executable exercises six committed revisions/events, view preferences, in-flight save acknowledgement, undo/redo, and exact source restoration. The second integration test reconstructs a host mirror from multi-change notifications in the declared coordinate space.

The API remains pre-1.0; these tests establish observable host behavior, not Rich layout or native input acceptance. Native composition and accessibility have their own surface and platform evidence in Phase 1B.
