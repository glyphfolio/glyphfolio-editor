# Original Markdown parser and projection

This egui-free crate is a disposable view of one named `editor-core` snapshot.
Persist the core snapshot, never `Projection::text()`. The engine does not edit
or serialize an AST back over the Markdown source.

`MarkdownParser::parse` performs full CommonMark reference parsing and returns a
cheaply cloned immutable `ParsedDocument`. `parse_change` additionally uses the
committed transaction's source anchors to preserve block identity, including
insertion among identical blocks. Without change metadata, full parsing can only
reconcile source-equivalent prefix/suffix positions.

Leaf blocks partition every original UTF-8 byte. Paragraph/heading/code metadata,
list/quote context, and nested inline styles are independent of the projection.
`BlockContext::list_item_start` distinguishes a list marker from continuation
paragraphs. Images, raw HTML, and reference definitions remain literal source
fallbacks rather than silently disappearing or executing external content.
Empty headings and fenced blocks without a body also keep their source visible:
there is no content anchor at which a folded caret could safely insert text.

`ParsedDocument::project(ProjectionOptions)` creates ordered Copy, Hidden, and
Transform segments in distinct source/display byte coordinates. Active blocks
are revealed with exact source spelling. Ambiguous caret boundaries use affinity;
outputs are legal extended-grapheme stops. A partial edit of transformed spelling
must reveal source or replace that whole segment. `projected_edit` creates an
original core transaction, validates snapshot identity, preserves unrelated bytes,
and requests grapheme-safe resulting selection. It does not apply the transaction.

The grammar is CommonMark 0.31.2 using pinned `pulldown-cmark` 0.13.5 with no
extensions. LF, CRLF and CR are normalized only in derived parser input; a sparse
delta map translates every event range back to original source bytes. This also
ensures a CRLF inside inline code is one CommonMark line ending. Saving/undoing
still preserves its exact two original bytes.

The conservative incremental path accepts only ASCII-word insertion/deletion in
an unstyled top-level plain paragraph. Structural/container/reference/Unicode or
inline-syntax edits use the full reference path. `ParseStats` reports source bytes
covered by grammar work, reused blocks and fallback reason. Full-source snapshot
copying and suffix range shifting remain O(N); no dirty-lines complexity promise
is made. Retained style indexes bound viewport overlap queries without a whole
document style scan.

Run:

```sh
cargo test --locked -p glyphfolio-editor-markdown
cargo run --locked --release -p glyphfolio-editor-markdown --example benchmark
```

The tests exercise all 652 official CommonMark examples plus projection coverage,
exact source reveal, Unicode/mixed endings, mapped edit/undo, stable identities,
reference invalidation, long inline-code resource bounds, and 300 deterministic
edit/undo differential cases. Expected HTML is tokenized to compare decoded
entities and structural content while ignoring formatting-only whitespace next
to block elements; inline and preformatted whitespace remain significant. This
does not claim that every construct already has a Rich UI renderer.

The separately licensed official test-data corpus is attributed in
`tests/fixtures/COMMONMARK-LICENSE.md`. All editor/projection behavior here is an
original implementation; parser and test-only HTML tokenizer dependencies are
low-level infrastructure.
