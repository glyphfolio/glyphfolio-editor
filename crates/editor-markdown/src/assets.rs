use glyphfolio_editor_core::ByteOffset;
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use std::ops::Range;
#[derive(Clone, Debug)]
pub struct ImageReference {
    pub source_range: Range<ByteOffset>,
    pub destination: String,
    pub alt: String,
}
/// Resolve syntax only; the caller owns all resource access.
pub fn image_references(source: &str) -> Vec<ImageReference> {
    let mut result = Vec::new();
    let mut active: Option<ImageReference> = None;
    for (event, range) in Parser::new(source).into_offset_iter() {
        match event {
            Event::Start(Tag::Image { dest_url, .. }) => {
                active = Some(ImageReference {
                    source_range: ByteOffset(range.start)..ByteOffset(range.end),
                    destination: dest_url.into_string(),
                    alt: String::new(),
                })
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some(reference) = &mut active {
                    reference.alt.push_str(&text);
                }
            }
            Event::End(TagEnd::Image) => {
                if let Some(reference) = active.take() {
                    result.push(reference);
                }
            }
            _ => {}
        }
    }
    result
}
