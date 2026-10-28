use glyphfolio_editor_core::{Affinity, Editor, Selection, TransactionOrigin};
use glyphfolio_editor_markdown::{
    DisplayOffset, MarkdownDialect, MarkdownParser, ProjectionOptions, editing,
};
use unicode_segmentation::UnicodeSegmentation;
#[test]
fn semantic_editing_refuses_foreign_revisions_without_touching_the_document() {
    let first = Editor::new("**text**");
    let parsed = MarkdownParser::with_dialect(MarkdownDialect::GfmWiki).parse(&first.snapshot());
    let view = parsed.project(ProjectionOptions::default());
    let mut foreign = Editor::new("**text**");
    assert!(
        editing::replace(
            &mut foreign,
            &view,
            parsed.styles(),
            0..1,
            "x",
            TransactionOrigin::Typing
        )
        .is_err()
    );
    assert!(editing::paragraph_break(&mut foreign, &view, false).is_err());
    assert!(editing::join_backward(&mut foreign, &view, parsed.styles()).is_err());
    assert_eq!(foreign.snapshot().text(), "**text**");
    assert!(!foreign.can_undo());
}
#[test]
fn visual_replacements_are_atomic_and_undo_exact_bytes_and_directed_selections() {
    let sources = [
        "**bold** plain",
        "*word* and ~~done~~",
        "**outer *inner***",
        "a &amp; b \\* c",
        "` code ` tail",
        "[label](https://example.org)",
        "| A | B |\n| --- | --- |\n| x | y |\n",
    ];
    for source in sources {
        let mut editor = Editor::new(source);
        let mut parser = MarkdownParser::with_dialect(MarkdownDialect::GfmWiki);
        let initial = parser
            .parse(&editor.snapshot())
            .project(ProjectionOptions::default());
        let boundaries: Vec<_> = initial
            .text()
            .grapheme_indices(true)
            .map(|(at, _)| at)
            .chain([initial.text().len()])
            .collect();
        for (i, start) in boundaries.iter().enumerate() {
            for end in &boundaries[i..] {
                let parsed = parser.parse(&editor.snapshot());
                let view = parsed.project(ProjectionOptions::default());
                let left = view
                    .display_to_source(DisplayOffset(*start), Affinity::After)
                    .unwrap();
                let right = view
                    .display_to_source(
                        DisplayOffset(*end),
                        if start == end {
                            Affinity::After
                        } else {
                            Affinity::Before
                        },
                    )
                    .unwrap();
                if left > right {
                    continue;
                }
                let selection = Selection {
                    anchor: right,
                    head: left,
                    affinity: Affinity::After,
                };
                editor.set_selection(selection).unwrap();
                let before = editor.snapshot();
                let result = editing::replace(
                    &mut editor,
                    &view,
                    parsed.styles(),
                    *start..*end,
                    "π",
                    TransactionOrigin::Typing,
                );
                if result.is_ok() {
                    editor.undo().unwrap().unwrap();
                    assert_eq!(editor.selection(), selection);
                } else {
                    assert_eq!(editor.snapshot().revision(), before.revision());
                }
                assert_eq!(editor.snapshot().text(), source);
                assert!(!editor.can_undo());
            }
        }
    }
}
