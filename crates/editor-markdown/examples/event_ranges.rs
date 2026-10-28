//! Inspect low-level CommonMark events and original byte ranges.
fn main() {
    for source in [
        "**a &amp; \\***\n",
        "> foo\n> bar\n",
        "- first\n  - nested\n- last\n",
        "``` rust\n x\n```\n",
        "a ` foo\r\n bar ` z\n",
        "[x][id]\n\n[id]: https://example.com\n",
    ] {
        println!("SOURCE {source:?}");
        for (event, range) in pulldown_cmark::Parser::new(source).into_offset_iter() {
            println!("{range:?} {:?} => {event:?}", &source[range.clone()]);
        }
    }
}
