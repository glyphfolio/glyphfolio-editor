use glyphfolio_editor_core::{Affinity, ByteOffset, Editor};
use glyphfolio_editor_markdown::{DisplayOffset, MarkdownParser, ProjectionOptions};
use unicode_segmentation::UnicodeSegmentation;

fn block_token(token: &html5gum::Token) -> bool {
    let name = match token {
        html5gum::Token::StartTag(tag) => &tag.name,
        html5gum::Token::EndTag(tag) => &tag.name,
        _ => return false,
    };
    matches!(
        name.as_ref(),
        b"address"
            | b"article"
            | b"aside"
            | b"blockquote"
            | b"body"
            | b"caption"
            | b"dd"
            | b"details"
            | b"div"
            | b"dl"
            | b"dt"
            | b"fieldset"
            | b"figcaption"
            | b"figure"
            | b"footer"
            | b"form"
            | b"h1"
            | b"h2"
            | b"h3"
            | b"h4"
            | b"h5"
            | b"h6"
            | b"head"
            | b"header"
            | b"hr"
            | b"html"
            | b"li"
            | b"main"
            | b"nav"
            | b"ol"
            | b"p"
            | b"pre"
            | b"section"
            | b"table"
            | b"tbody"
            | b"td"
            | b"tfoot"
            | b"th"
            | b"thead"
            | b"tr"
            | b"ul"
    )
}

fn canonical_html(html: &str) -> Vec<html5gum::Token> {
    let tokens: Vec<_> = html5gum::Tokenizer::new(html)
        .filter_map(Result::ok)
        .collect();
    let mut output = Vec::new();
    let mut preserved = 0_usize;
    for (index, token) in tokens.iter().enumerate() {
        let preserve_start = matches!(token, html5gum::Token::StartTag(tag) if matches!(tag.name.as_ref(), b"pre" | b"code" | b"textarea" | b"script" | b"style"));
        if preserve_start {
            preserved += 1;
        }
        let formatting_space = preserved == 0
            && matches!(token, html5gum::Token::String(text) if text.value.iter().all(u8::is_ascii_whitespace))
            && (index > 0 && block_token(&tokens[index - 1])
                || tokens.get(index + 1).is_some_and(block_token));
        if !formatting_space {
            output.push(token.clone());
        }
        if matches!(token, html5gum::Token::EndTag(tag) if matches!(tag.name.as_ref(), b"pre" | b"code" | b"textarea" | b"script" | b"style"))
        {
            preserved = preserved.saturating_sub(1);
        }
    }
    output
}

#[test]
fn html_comparison_preserves_inline_and_preformatted_whitespace() {
    assert_eq!(
        canonical_html("<p>\"</p>\n"),
        canonical_html("<p>&quot;</p>\n")
    );
    assert_eq!(
        canonical_html("<ul><li><div>\n</li></ul>"),
        canonical_html("<ul>\n<li>\n<div>\n</li>\n</ul>\n")
    );
    assert_ne!(
        canonical_html("<p><em>a</em> <em>b</em></p>"),
        canonical_html("<p><em>a</em><em>b</em></p>")
    );
    assert_ne!(
        canonical_html("<pre> \n</pre>"),
        canonical_html("<pre>\n</pre>")
    );
}

#[test]
fn official_commonmark_0312_grammar_and_projection_corpus() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/commonmark-0.31.2.json")).unwrap();
    let cases = cases.as_array().unwrap();
    assert_eq!(cases.len(), 652);
    for case in cases {
        let source = case["markdown"].as_str().unwrap();
        let expected = case["html"].as_str().unwrap();
        let example = case["example"].as_u64().unwrap();
        let editor = Editor::new(source);
        let document = MarkdownParser::new().parse(&editor.snapshot());
        // HTML permits literal quotes in text nodes as well as &quot;.
        // Compare decoded HTML tokens, retaining tags/attributes/text structure,
        // rather than requiring one serializer's redundant escaping choices.
        let rendered = document.reference_html();
        let actual = canonical_html(&rendered);
        let expected_tokens = canonical_html(expected);
        assert_eq!(
            actual, expected_tokens,
            "official CommonMark example {example}: {rendered:?} != {expected:?}"
        );
        assert_eq!(document.source(), source);
        let projection = document.project(ProjectionOptions::default());
        let mut end = 0;
        for block in document.blocks() {
            assert_eq!(
                block.source_range.start.0, end,
                "block coverage example {example}"
            );
            end = block.source_range.end.0;
        }
        assert_eq!(end, source.len());
        let mut source_end = 0;
        let mut display_end = 0;
        for segment in projection.segments() {
            assert_eq!(
                segment.source_range.start.0, source_end,
                "segment source coverage example {example}"
            );
            assert_eq!(
                segment.display_range.start.0, display_end,
                "segment display coverage example {example}"
            );
            source_end = segment.source_range.end.0;
            display_end = segment.display_range.end.0;
        }
        assert_eq!(source_end, source.len());
        assert_eq!(display_end, projection.text().len());
        for (offset, _) in projection
            .text()
            .grapheme_indices(true)
            .chain([(projection.text().len(), "")])
        {
            for affinity in [Affinity::Before, Affinity::After] {
                let source_offset = projection
                    .display_to_source(DisplayOffset(offset), affinity)
                    .unwrap();
                assert!(source.is_char_boundary(source_offset.0));
                let display = projection
                    .source_to_display(source_offset, affinity)
                    .unwrap();
                assert!(projection.text().is_char_boundary(display.0));
            }
        }
        let revealed = document.project(ProjectionOptions {
            reveal: Some(ByteOffset(0)..ByteOffset(source.len())),
        });
        assert_eq!(
            revealed.text(),
            source,
            "full source reveal example {example}"
        );
    }
}
