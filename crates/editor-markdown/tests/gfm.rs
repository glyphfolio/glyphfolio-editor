use glyphfolio_editor_core::{ByteOffset, Editor};
use glyphfolio_editor_markdown::{MarkdownDialect, MarkdownParser, ProjectionOptions, StyleKind};
#[test]
fn gfm_is_explicit_and_source_projection_is_lossless_on_reveal() {
    let source = "~~old **bold**~~\n\n- [ ] task\n- [x] done\n\n[[Note title|日本]] and `[[literal]]`\n\n| A | B |\n| --- | --- |\n| x | y |\n";
    let editor = Editor::new(source);
    let document = MarkdownParser::with_dialect(MarkdownDialect::GfmWiki).parse(&editor.snapshot());
    let projection = document.project(ProjectionOptions::default());
    assert!(projection.text().contains("old bold"));
    assert!(projection.text().contains("☐ task"));
    assert!(projection.text().contains("☑ done"));
    assert!(projection.text().contains("日本 and [[literal]]"));
    assert!(projection.styles().iter().any(|span| matches!(&span.kind,StyleKind::Link{destination,..} if destination=="wiki:Note title")));
    assert!(document.reference_html().contains("<table>"));
    assert_eq!(
        document
            .project(ProjectionOptions {
                reveal: Some(ByteOffset(0)..ByteOffset(source.len()))
            })
            .text(),
        source
    );
    let common = MarkdownParser::new().parse(&editor.snapshot());
    assert!(!common.reference_html().contains("<del>"));
}
#[test]
fn wiki_links_do_not_reinterpret_code_html_images_or_existing_links() {
    for source in [
        "`[[code]]`",
        "<div>[[html]]</div>",
        "![image [[alt]]](image.png)",
        "[[[inside]]](https://example.org)",
        "\\[[escaped]]",
    ] {
        let doc = MarkdownParser::with_dialect(MarkdownDialect::GfmWiki)
            .parse(&Editor::new(source).snapshot());
        assert!(!doc.styles().iter().any(|span|matches!(&span.kind,StyleKind::Link{destination,..} if destination.starts_with("wiki:"))),"{source}");
    }
}

#[test]
fn table_cell_styles_entities_and_escaped_pipes_keep_a_partitioned_source_map() {
    use glyphfolio_editor_markdown::SegmentKind;
    let source = "| **Name** | Value |\r\n| --- | --- |\r\n| 日本 | a\\|b &amp; |\r\n";
    let doc = MarkdownParser::with_dialect(MarkdownDialect::GfmWiki)
        .parse(&Editor::new(source).snapshot());
    let projection = doc.project(ProjectionOptions::default());
    assert!(projection.text().contains("Name"));
    assert!(!projection.text().contains("**"));
    assert!(projection.text().contains("a|b &"));
    let mut end = 0;
    for segment in projection.segments() {
        assert_eq!(segment.source_range.start.0, end);
        end = segment.source_range.end.0;
        if segment.kind == SegmentKind::Copy {
            assert_eq!(
                &source[segment.source_range.start.0..segment.source_range.end.0],
                &projection.text()[segment.display_range.start.0..segment.display_range.end.0]
            );
        }
    }
    assert_eq!(end, source.len());
    assert_eq!(
        doc.project(ProjectionOptions {
            reveal: Some(ByteOffset(0)..ByteOffset(source.len()))
        })
        .text(),
        source
    );
}

#[test]
fn unsupported_table_shapes_keep_every_source_byte_visible() {
    for source in [
        "| A | B |\n| --- | --- |\n| only |\n",
        "A | B\n--- | ---\none value\n",
    ] {
        let doc = MarkdownParser::with_dialect(MarkdownDialect::GfmWiki)
            .parse(&Editor::new(source).snapshot());
        let projection = doc.project(ProjectionOptions::default());
        assert_eq!(projection.text(), source);
        assert_eq!(
            projection.segments().last().unwrap().source_range.end.0,
            source.len()
        );
    }
}

#[test]
fn native_fixture_table_grid_keeps_all_column_separators() {
    let source = include_str!("../../../examples/standalone-egui/fixtures/editing-parity.md");
    let doc = MarkdownParser::with_dialect(MarkdownDialect::GfmWiki)
        .parse(&Editor::new(source).snapshot());
    let projection = doc.project(ProjectionOptions::default());
    let row = projection
        .text()
        .lines()
        .find(|line| line.contains("Tables"))
        .unwrap();
    assert_eq!(row, "Tables\tEditable");
    assert!(!projection.text().contains('│'));
}

#[test]
fn rich_cell_breaks_are_layout_newlines_while_source_and_commonmark_stay_literal() {
    let source = "| A | B |\n| --- | --- |\n| one<br>two | x |\n";
    let parsed = MarkdownParser::with_dialect(MarkdownDialect::GfmWiki)
        .parse(&Editor::new(source).snapshot());
    let projection = parsed.project(ProjectionOptions::default());
    assert!(projection.text().contains("one\ntwo"));
    assert!(!projection.text().contains("<br>"));
    assert_eq!(
        parsed
            .project(ProjectionOptions {
                reveal: Some(ByteOffset(0)..ByteOffset(source.len()))
            })
            .text(),
        source
    );
}
