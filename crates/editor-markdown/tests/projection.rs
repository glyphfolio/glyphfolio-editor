use glyphfolio_editor_core::{
    Affinity, ByteOffset, Editor, Selection, Transaction, TransactionOrigin,
};
use glyphfolio_editor_markdown::{
    BlockKind, DisplayOffset, ListKind, MarkdownParser, ParseMode, ProjectionError,
    ProjectionOptions, SegmentKind, StyleKind,
};
use unicode_segmentation::UnicodeSegmentation;

fn parsed(text: &str) -> (Editor, glyphfolio_editor_markdown::ParsedDocument) {
    let editor = Editor::new(text);
    let document = MarkdownParser::new().parse(&editor.snapshot());
    (editor, document)
}

fn assert_partition(document: &glyphfolio_editor_markdown::ParsedDocument) {
    let mut previous = 0;
    for block in document.blocks() {
        assert_eq!(block.source_range.start.0, previous);
        assert!(
            document
                .source()
                .is_char_boundary(block.source_range.start.0)
        );
        assert!(document.source().is_char_boundary(block.source_range.end.0));
        previous = block.source_range.end.0;
    }
    assert_eq!(previous, document.source().len());
    let projection = document.project(ProjectionOptions::default());
    let mut source = 0;
    let mut display = 0;
    for segment in projection.segments() {
        assert_eq!(segment.source_range.start.0, source);
        assert_eq!(segment.display_range.start.0, display);
        assert!(
            projection
                .text()
                .is_char_boundary(segment.display_range.start.0)
        );
        assert!(
            projection
                .text()
                .is_char_boundary(segment.display_range.end.0)
        );
        if segment.kind == SegmentKind::Copy {
            assert_eq!(
                &document.source()[segment.source_range.start.0..segment.source_range.end.0],
                &projection.text()[segment.display_range.start.0..segment.display_range.end.0]
            );
        }
        source = segment.source_range.end.0;
        display = segment.display_range.end.0;
    }
    assert_eq!(source, document.source().len());
    assert_eq!(display, projection.text().len());
    for (at, _) in projection
        .text()
        .grapheme_indices(true)
        .chain([(projection.text().len(), "")])
    {
        for affinity in [Affinity::Before, Affinity::After] {
            let source = projection
                .display_to_source(DisplayOffset(at), affinity)
                .unwrap();
            assert!(document.source().is_char_boundary(source.0));
            assert!(projection.source_to_display(source, affinity).is_ok());
        }
    }
    let revealed = document.project(ProjectionOptions {
        reveal: Some(ByteOffset(0)..ByteOffset(document.source().len())),
    });
    assert_eq!(revealed.text(), document.source());
}

#[test]
fn nested_styles_entities_escapes_links_and_code_keep_original_source() {
    let source = "# Title **bold**\n\n*em [label **strong**](https://example.com \"title\")* &amp; \\* ` code `\n";
    let (_, document) = parsed(source);
    assert_eq!(document.source(), source);
    let projection = document.project(ProjectionOptions::default());
    assert_eq!(
        projection.text(),
        "Title bold\n\nem label strong & * code\n"
    );
    assert!(
        document
            .blocks()
            .iter()
            .any(|block| block.kind == BlockKind::Heading(1))
    );
    assert!(
        projection
            .styles()
            .iter()
            .any(|span| matches!(span.kind, StyleKind::Link { .. }))
    );
    assert!(
        projection
            .segments()
            .iter()
            .any(|segment| segment.kind == SegmentKind::Transform)
    );
    assert_partition(&document);
}

#[test]
fn empty_structures_keep_source_visible_instead_of_inventing_a_content_anchor() {
    for source in ["#", "## \n", "```\n```", "```rust\n```\n"] {
        let (_, document) = parsed(source);
        let projection = document.project(ProjectionOptions::default());
        assert_eq!(projection.text(), source);
        for offset in 0..=source.len() {
            assert_eq!(
                projection
                    .display_to_source(DisplayOffset(offset), Affinity::After)
                    .unwrap(),
                ByteOffset(offset)
            );
        }
        assert_partition(&document);
    }
    // A real empty body line supplies its own source anchor.
    let (_, document) = parsed("```rust\n\n```\n");
    assert_eq!(document.project(ProjectionOptions::default()).text(), "\n");
}

#[test]
fn nested_list_quote_fence_context_and_all_source_bytes_are_retained() {
    let source = "> 3. first\n>    - nested\n> 4. last\n\n```rust\nlet n = 1;\n```\n\n![alt](asset.png)\n\n[id]: /target\n";
    let (_, document) = parsed(source);
    assert_partition(&document);
    let first = document
        .blocks()
        .iter()
        .find(|block| {
            &source[block.source_range.start.0..block.source_range.end.0] == "> 3. first\n"
        })
        .unwrap();
    assert_eq!(first.context.quote_depth, 1);
    assert_eq!(
        first.context.list_kind,
        Some(ListKind::Ordered { start: 3 })
    );
    assert_eq!(first.context.item_number, Some(3));
    assert!(
        document
            .blocks()
            .iter()
            .any(|block| block.context.list_depth == 2)
    );
    let projection = document.project(ProjectionOptions::default());
    assert!(projection.text().contains("first\nnested\nlast\n"));
    assert!(projection.text().contains("let n = 1;\n"));
    assert!(projection.text().contains("![alt](asset.png)"));
    assert!(projection.text().contains("[id]: /target"));
}

#[test]
fn hidden_boundaries_use_affinity_and_active_block_reveal_is_exact() {
    let (_, document) = parsed("**a**\n\nnext");
    let projection = document.project(ProjectionOptions::default());
    assert_eq!(
        projection
            .display_to_source(DisplayOffset(0), Affinity::Before)
            .unwrap(),
        ByteOffset(0)
    );
    assert_eq!(
        projection
            .display_to_source(DisplayOffset(0), Affinity::After)
            .unwrap(),
        ByteOffset(2)
    );
    assert_eq!(
        projection
            .display_to_source(DisplayOffset(1), Affinity::Before)
            .unwrap(),
        ByteOffset(3)
    );
    assert_eq!(
        projection
            .display_to_source(DisplayOffset(1), Affinity::After)
            .unwrap(),
        ByteOffset(5)
    );
    let revealed = document.project(ProjectionOptions {
        reveal: Some(ByteOffset(2)..ByteOffset(2)),
    });
    assert_eq!(revealed.text(), "**a**\n\nnext");
    assert!(revealed.blocks()[0].revealed);
    assert!(!revealed.blocks().last().unwrap().revealed);
    assert!(
        revealed
            .styles()
            .iter()
            .any(|span| span.kind == StyleKind::Syntax)
    );
}

#[test]
fn entity_partial_edits_require_reveal_but_whole_edits_are_atomic_and_undoable() {
    let (mut editor, document) = parsed("**&fjlig;** &amp; e\u{301}");
    let projection = document.project(ProjectionOptions::default());
    assert_eq!(projection.text(), "fj & e\u{301}");
    assert!(matches!(
        projection.projected_edit(
            &editor.snapshot(),
            DisplayOffset(1)..DisplayOffset(1),
            "x",
            Affinity::After,
            TransactionOrigin::Typing
        ),
        Err(ProjectionError::RevealRequired { .. })
    ));
    assert!(!editor.can_undo());
    let transaction = projection
        .projected_edit(
            &editor.snapshot(),
            DisplayOffset(0)..DisplayOffset(2),
            "q",
            Affinity::After,
            TransactionOrigin::Typing,
        )
        .unwrap();
    editor.apply(transaction).unwrap();
    assert_eq!(editor.snapshot().text(), "**q** &amp; e\u{301}");
    editor.undo().unwrap();
    assert_eq!(editor.snapshot().text(), "**&fjlig;** &amp; e\u{301}");
    assert!(matches!(
        projection.projected_edit(
            &editor.snapshot(),
            DisplayOffset(0)..DisplayOffset(0),
            "x",
            Affinity::After,
            TransactionOrigin::Typing
        ),
        Err(ProjectionError::StaleSnapshot)
    ));
}

#[test]
fn display_graphemes_and_foreign_snapshot_are_checked_before_editing() {
    let (editor, document) = parsed("a&#x301; 👩‍👩‍👧‍👦");
    let projection = document.project(ProjectionOptions::default());
    assert_eq!(projection.text(), "a\u{301} 👩‍👩‍👧‍👦");
    assert!(matches!(
        projection.display_to_source(DisplayOffset(1), Affinity::After),
        Err(ProjectionError::NotGraphemeBoundary(_))
    ));
    assert!(matches!(
        projection.display_to_source(DisplayOffset(2), Affinity::After),
        Err(ProjectionError::InvalidDisplayOffset(_))
    ));
    assert_eq!(
        projection
            .source_to_display(ByteOffset(1), Affinity::Before)
            .unwrap(),
        DisplayOffset(0)
    );
    assert_eq!(
        projection
            .source_to_display(ByteOffset(1), Affinity::After)
            .unwrap(),
        DisplayOffset(3)
    );
    let foreign = Editor::new(&editor.snapshot().text());
    assert!(matches!(
        projection.projected_edit(
            &foreign.snapshot(),
            DisplayOffset(0)..DisplayOffset(0),
            "x",
            Affinity::After,
            TransactionOrigin::Typing
        ),
        Err(ProjectionError::StaleSnapshot)
    ));
    assert_partition(&document);
}

#[test]
fn malformed_and_unsupported_constructs_remain_source_editable_without_panics() {
    for source in [
        "",
        "***",
        "[missing](",
        "<script>alert(1)</script>\n",
        "\u{feff}# BOM\r\n\r\n~~~x\r\nbody\r\n",
        "-\n- \n",
        "a ` foo\r\n bar ` z\n",
        "a  \r\nb\\\nc\n",
        "``\n`\n",
    ] {
        let (_, document) = parsed(source);
        assert_partition(&document);
    }
}

#[test]
fn stable_blocks_and_reference_style_generation_survive_unrelated_edits() {
    let mut editor = Editor::new("[same][id]\n\nlast paragraph\n\n[id]: /one\n");
    let mut parser = MarkdownParser::new();
    let before = parser.parse(&editor.snapshot());
    let before_projection = before.project(ProjectionOptions::default());
    let at = before.source().find("/one").unwrap();
    editor
        .apply(
            Transaction::new(&editor.snapshot())
                .replace(ByteOffset(at)..ByteOffset(at + 4), "/two"),
        )
        .unwrap();
    let after = parser.parse(&editor.snapshot());
    let after_projection = after.project(ProjectionOptions::default());
    assert_eq!(before.blocks()[0].id, after.blocks()[0].id);
    assert_ne!(
        before_projection.blocks()[0].generation,
        after_projection.blocks()[0].generation
    );
    assert_eq!(
        before_projection.blocks()[2].generation,
        after_projection.blocks()[2].generation
    );
    let identical = parser.parse(&editor.snapshot());
    assert_eq!(identical.stats().mode, ParseMode::Unchanged);
    assert_eq!(identical.stats().parsed_bytes, 0);
}

#[test]
fn paragraph_incremental_path_matches_full_reference_after_generated_edits() {
    let initial = (0..500)
        .map(|index| format!("paragraph {index} words\n\n"))
        .collect::<String>();
    let mut editor = Editor::new(&initial);
    let mut parser = MarkdownParser::new();
    parser.parse(&editor.snapshot());
    let mut seed = 17_u64;
    for step in 0..100 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let current = editor.snapshot().text();
        let paragraph = (seed as usize) % 500;
        let marker = format!("paragraph {paragraph} ");
        let at = current.find(&marker).unwrap() + marker.len();
        let inserted = if step % 4 == 0 { "Z" } else { "a" };
        let change = editor
            .apply(
                Transaction::new(&editor.snapshot())
                    .replace(ByteOffset(at)..ByteOffset(at), inserted),
            )
            .unwrap();
        let incremental = parser.parse_change(&change);
        let full = MarkdownParser::new().parse(&editor.snapshot());
        assert_eq!(incremental.stats().mode, ParseMode::IncrementalParagraph);
        assert!(incremental.stats().parsed_bytes < 100);
        assert_eq!(incremental.source(), full.source());
        assert_eq!(
            incremental
                .blocks()
                .iter()
                .map(|block| (&block.kind, &block.context, &block.source_range))
                .collect::<Vec<_>>(),
            full.blocks()
                .iter()
                .map(|block| (&block.kind, &block.context, &block.source_range))
                .collect::<Vec<_>>()
        );
        let projected = incremental.project(ProjectionOptions::default());
        let reference = full.project(ProjectionOptions::default());
        assert_eq!(projected.text(), reference.text());
        assert_eq!(projected.segments(), reference.segments());
        assert_eq!(projected.styles(), reference.styles());
    }
    let change = editor
        .apply(Transaction::new(&editor.snapshot()).replace(ByteOffset(0)..ByteOffset(0), "```\n"))
        .unwrap();
    let result = parser.parse_change(&change);
    assert_eq!(result.stats().mode, ParseMode::Full);
    assert_eq!(
        result.reference_html(),
        MarkdownParser::new()
            .parse(&editor.snapshot())
            .reference_html()
    );
}

#[test]
fn projection_edit_into_plain_copy_preserves_every_unrelated_markdown_byte() {
    let source = "\u{feff}# Header\r\n\r\n**hello** &amp; tail  \r\n";
    let (mut editor, document) = parsed(source);
    let projection = document.project(ProjectionOptions::default());
    let at = projection.text().find("hello").unwrap() + 2;
    let transaction = projection
        .projected_edit(
            &editor.snapshot(),
            DisplayOffset(at)..DisplayOffset(at),
            "日",
            Affinity::After,
            TransactionOrigin::Typing,
        )
        .unwrap();
    editor.apply(transaction).unwrap();
    assert_eq!(editor.snapshot().text(), source.replace("hello", "he日llo"));
    editor.undo().unwrap();
    assert_eq!(editor.snapshot().text().as_bytes(), source.as_bytes());
    assert_eq!(editor.selection(), Selection::default());
}

#[test]
fn inserted_duplicate_block_does_not_take_an_existing_sources_identity() {
    let mut editor = Editor::new("same\n\nsame\n");
    let mut parser = MarkdownParser::new();
    let before = parser.parse(&editor.snapshot());
    let old: Vec<_> = before
        .blocks()
        .iter()
        .filter(|block| block.kind == BlockKind::Paragraph)
        .map(|block| block.id)
        .collect();
    let change = editor
        .apply(
            Transaction::new(&editor.snapshot()).replace(ByteOffset(0)..ByteOffset(0), "same\n\n"),
        )
        .unwrap();
    let after = parser.parse_change(&change);
    let new: Vec<_> = after
        .blocks()
        .iter()
        .filter(|block| block.kind == BlockKind::Paragraph)
        .map(|block| block.id)
        .collect();
    assert_ne!(new[0], old[0]);
    assert_eq!(&new[1..], old.as_slice());
    let foreign = Editor::new("same\n\nsame\n");
    let foreign = parser.parse(&foreign.snapshot());
    assert!(!foreign.blocks().iter().any(|block| {
        after
            .blocks()
            .iter()
            .any(|previous| previous.id == block.id)
    }));
}

#[test]
fn structural_reference_and_unicode_changes_fall_back_and_match_full_parse() {
    let mut editor = Editor::new("[linked][id]\n\nplain words\n\n[id]: /old\n");
    let mut parser = MarkdownParser::new();
    parser.parse(&editor.snapshot());
    for (needle, replacement) in [
        ("/old", "/new"),
        ("plain", "**plain**"),
        ("words", "日本"),
        ("[linked][id]", "[linked](elsewhere)"),
    ] {
        let source = editor.snapshot().text();
        let at = source.find(needle).unwrap();
        let change = editor
            .apply(
                Transaction::new(&editor.snapshot())
                    .replace(ByteOffset(at)..ByteOffset(at + needle.len()), replacement),
            )
            .unwrap();
        let result = parser.parse_change(&change);
        let full = MarkdownParser::new().parse(&editor.snapshot());
        assert_eq!(result.stats().mode, ParseMode::Full);
        assert_eq!(result.reference_html(), full.reference_html());
        assert_eq!(
            result.project(ProjectionOptions::default()).text(),
            full.project(ProjectionOptions::default()).text()
        );
        assert_eq!(result.styles(), full.styles());
        assert_partition(&result);
    }
}

#[test]
fn normalized_code_crlf_and_hidden_delimiter_graphemes_cannot_be_partly_corrupted() {
    let (mut editor, document) = parsed("`foo\r\nbar`");
    let projection = document.project(ProjectionOptions::default());
    assert_eq!(projection.text(), "foo bar");
    let transaction = projection
        .projected_edit(
            &editor.snapshot(),
            DisplayOffset(3)..DisplayOffset(4),
            "",
            Affinity::After,
            TransactionOrigin::Delete,
        )
        .unwrap();
    editor.apply(transaction).unwrap();
    assert_eq!(editor.snapshot().text(), "`foobar`");
    editor.undo().unwrap();
    assert_eq!(editor.snapshot().text(), "`foo\r\nbar`");
    let (editor, document) = parsed("`\u{301}a`");
    let projection = document.project(ProjectionOptions::default());
    assert_eq!(projection.text(), "\u{301}a");
    assert!(matches!(
        projection.projected_edit(
            &editor.snapshot(),
            DisplayOffset(0)..DisplayOffset(0),
            "x",
            Affinity::After,
            TransactionOrigin::Typing
        ),
        Err(ProjectionError::RevealRequired { .. })
    ));
}

#[test]
fn commonmark_line_endings_have_equal_semantics_and_exact_original_ranges() {
    let source = "# 日本\n\n> a\n> b\n\n`foo\nbar`\n\n```x\nline1\nline2\n```\n";
    let (_, baseline) = parsed(source);
    for ending in ["\r\n", "\r"] {
        let converted = source.replace('\n', ending);
        let (_, document) = parsed(&converted);
        assert_eq!(document.reference_html(), baseline.reference_html());
        assert_eq!(
            document.project(ProjectionOptions::default()).text(),
            baseline.project(ProjectionOptions::default()).text()
        );
        assert_partition(&document);
    }
}

#[test]
fn loose_list_continuations_keep_indent_without_another_bullet() {
    let (_, document) = parsed("- first\n\n  continuation\n\n- second\n");
    let paragraphs: Vec<_> = document
        .blocks()
        .iter()
        .filter(|block| block.kind == BlockKind::Paragraph)
        .collect();
    assert_eq!(paragraphs.len(), 3);
    assert!(paragraphs[0].context.list_item_start);
    assert!(!paragraphs[1].context.list_item_start);
    assert_eq!(paragraphs[1].context.list_depth, 1);
    assert!(paragraphs[2].context.list_item_start);
    assert_partition(&document);
}

#[test]
fn projected_typing_snaps_the_caret_after_newly_joined_graphemes() {
    for (source, at, inserted, expected) in [
        ("\u{301}", 0, "a", "a\u{301}"),
        ("👩👩", 4, "\u{200d}", "👩‍👩"),
    ] {
        let (mut editor, document) = parsed(source);
        let projection = document.project(ProjectionOptions::default());
        let transaction = projection
            .projected_edit(
                &editor.snapshot(),
                DisplayOffset(at)..DisplayOffset(at),
                inserted,
                Affinity::After,
                TransactionOrigin::Typing,
            )
            .unwrap();
        editor.apply(transaction).unwrap();
        assert_eq!(editor.snapshot().text(), expected);
        assert_eq!(editor.selection().head, ByteOffset(expected.len()));
        editor.undo().unwrap();
        assert_eq!(editor.snapshot().text(), source);
    }
}

#[test]
fn indexed_viewport_styles_equal_nested_interval_semantics() {
    let (_, document) = parsed("**outer *inner* tail**\n\nplain **late**");
    let projection = document.project(ProjectionOptions::default());
    for start in 0..=projection.text().len() {
        let end = (start + 3).min(projection.text().len());
        let expected: Vec<_> = projection
            .styles()
            .iter()
            .filter(|style| {
                style.display_range.start.0 < end
                    && start < style.display_range.end.0
                    && start < end
            })
            .collect();
        let indexed: Vec<_> = projection
            .styles_intersecting(DisplayOffset(start)..DisplayOffset(end))
            .collect();
        assert_eq!(indexed, expected);
    }
    assert!(
        projection
            .styles_intersecting(DisplayOffset(2)..DisplayOffset(2))
            .next()
            .is_none()
    );
}

#[test]
fn generated_structural_edits_and_undo_match_full_parse_and_projection() {
    let mut editor = Editor::new("# heading\n\n> quoted **word**\n\n[link][id]\n\n[id]: /target\n");
    let mut parser = MarkdownParser::new();
    parser.parse(&editor.snapshot());
    let tokens = [
        "a", "*", "_", "`", "\n", "> ", "- ", "[", "](", ")", "&amp;", "日", "\u{301}", "\\",
        "\r\n",
    ];
    let mut seed = 0x8ae13_u64;
    for step in 0..200 {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let change = if step % 11 == 0 && editor.can_undo() {
            editor.undo().unwrap().unwrap()
        } else {
            let source = editor.snapshot().text();
            let offsets: Vec<_> = source
                .char_indices()
                .map(|(offset, _)| offset)
                .chain([source.len()])
                .collect();
            let index = seed as usize % offsets.len();
            let start = offsets[index];
            let end = if step % 3 == 0 {
                offsets[(index + 1).min(offsets.len() - 1)]
            } else {
                start
            };
            let candidate = tokens[(seed >> 32) as usize % tokens.len()];
            let token = if &source[start..end] == candidate {
                "Ω"
            } else {
                candidate
            };
            editor
                .apply(
                    Transaction::new(&editor.snapshot())
                        .replace(ByteOffset(start)..ByteOffset(end), token),
                )
                .unwrap()
        };
        let result = parser.parse_change(&change);
        let reference = MarkdownParser::new().parse(&editor.snapshot());
        assert_eq!(
            result.reference_html(),
            reference.reference_html(),
            "step {step}"
        );
        assert_eq!(result.styles(), reference.styles(), "step {step}");
        let display = result.project(ProjectionOptions::default());
        let expected = reference.project(ProjectionOptions::default());
        assert_eq!(display.text(), expected.text(), "step {step}");
        assert_eq!(display.segments(), expected.segments(), "step {step}");
        assert_partition(&result);
    }
}
