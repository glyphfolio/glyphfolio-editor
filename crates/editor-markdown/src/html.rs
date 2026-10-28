//! Bounded, original clipboard conversion. HTML is data: never executed or fetched.
use html5gum::{Token, Tokenizer};

#[derive(Default)]
struct Node {
    tag: String,
    text: String,
    href: Option<String>,
    children: Vec<usize>,
}

/// Convert clipboard HTML into GFM source. Reject oversized/deep inputs so a host
/// can explicitly fall back to plain text. Resource-bearing content is excluded.
pub fn html_to_markdown(html: &str) -> Result<String, &'static str> {
    if html.len() > 2 * 1024 * 1024 {
        return Err("HTML paste exceeds 2 MiB");
    }
    let mut nodes = vec![Node::default()];
    let mut stack = vec![0usize];
    for token in Tokenizer::new(html).flatten() {
        match token {
            Token::StartTag(tag) => {
                if nodes.len() >= 100_000 || stack.len() >= 128 {
                    return Err("HTML paste is too complex");
                }
                let name = String::from_utf8_lossy(&tag.name).to_ascii_lowercase();
                let href = tag
                    .attributes
                    .iter()
                    .find(|(key, _)| key.as_ref() == b"href")
                    .map(|(_, value)| String::from_utf8_lossy(&value.value).into_owned());
                let id = nodes.len();
                nodes.push(Node {
                    tag: name.clone(),
                    href,
                    ..Node::default()
                });
                nodes[*stack.last().expect("root")].children.push(id);
                if !tag.self_closing
                    && !matches!(
                        name.as_str(),
                        "br" | "hr"
                            | "img"
                            | "input"
                            | "meta"
                            | "link"
                            | "source"
                            | "wbr"
                            | "area"
                            | "base"
                            | "embed"
                            | "param"
                            | "track"
                            | "col"
                    )
                {
                    stack.push(id);
                }
            }
            Token::EndTag(tag) => {
                let name = String::from_utf8_lossy(&tag.name);
                if let Some(at) = stack.iter().rposition(|id| nodes[*id].tag == name) {
                    stack.truncate(at.max(1));
                }
            }
            Token::String(text) => {
                let id = nodes.len();
                if id >= 100_000 {
                    return Err("HTML paste is too complex");
                }
                nodes.push(Node {
                    text: String::from_utf8_lossy(&text.value).into_owned(),
                    ..Node::default()
                });
                nodes[*stack.last().expect("root")].children.push(id);
            }
            _ => {}
        }
    }
    Ok(render(&nodes, 0).trim().to_owned())
}

/// Host link commands use the same allowlist as formatted paste. Relative note
/// and attachment references are allowed; executable/data/file schemes are not.
pub fn safe_link(destination: &str) -> bool {
    let value = destination.trim();
    if value.is_empty()
        || value.chars().any(char::is_control)
        || value.starts_with("//")
        || value.contains('\\')
    {
        return false;
    }
    let first = value.split(['/', '?', '#']).next().unwrap_or(value);
    if let Some((scheme, _)) = first.split_once(':') {
        matches!(
            scheme.to_ascii_lowercase().as_str(),
            "http" | "https" | "mailto" | "tel"
        )
    } else {
        true
    }
}

fn escape(text: &str) -> String {
    let mut output = String::new();
    let mut space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !space {
                output.push(' ');
            }
            space = true;
        } else {
            space = false;
            if "\\`*_{}[]<>#!|~+-.=".contains(ch) {
                output.push('\\');
            }
            output.push(ch);
        }
    }
    output
}
fn raw(nodes: &[Node], id: usize) -> String {
    let node = &nodes[id];
    let mut output = node.text.clone();
    for child in &node.children {
        output.push_str(&raw(nodes, *child));
    }
    output
}
fn marked(content: &str, mark: &str) -> String {
    let begin = content.len() - content.trim_start().len();
    let end = content.trim_end().len();
    if begin >= end {
        return content.into();
    }
    format!(
        "{}{mark}{}{mark}{}",
        &content[..begin],
        &content[begin..end],
        &content[end..]
    )
}
fn render(nodes: &[Node], id: usize) -> String {
    let node = &nodes[id];
    if matches!(
        node.tag.as_str(),
        "script"
            | "style"
            | "iframe"
            | "object"
            | "embed"
            | "svg"
            | "math"
            | "head"
            | "template"
            | "noscript"
            | "img"
            | "video"
            | "audio"
            | "input"
    ) {
        return String::new();
    }
    if node.tag.is_empty() && !node.text.is_empty() {
        return escape(&node.text);
    }
    if node.tag == "pre" {
        let text = raw(nodes, id);
        let fence =
            "`".repeat(3.max(text.split(|ch| ch != '`').map(str::len).max().unwrap_or(0) + 1));
        return format!(
            "\n\n{fence}\n{}{fence}\n\n",
            if text.ends_with('\n') {
                text
            } else {
                format!("{text}\n")
            }
        );
    }
    if node.tag == "code" {
        let text = raw(nodes, id).replace(['\r', '\n'], " ");
        let fence = "`".repeat(1 + text.split(|ch| ch != '`').map(str::len).max().unwrap_or(0));
        let padding = if text.starts_with('`')
            || text.ends_with('`')
            || (text.starts_with(' ') && text.ends_with(' '))
        {
            " "
        } else {
            ""
        };
        return format!("{fence}{padding}{text}{padding}{fence}");
    }
    if matches!(node.tag.as_str(), "ul" | "ol") {
        let mut output = String::from("\n");
        let mut number = 1;
        for child in &node.children {
            if nodes[*child].tag != "li" {
                continue;
            }
            let content = render(nodes, *child);
            let marker = if node.tag == "ol" {
                format!("{number}. ")
            } else {
                "- ".into()
            };
            let mut lines = content.trim().lines();
            output.push_str(&marker);
            output.push_str(lines.next().unwrap_or(""));
            output.push('\n');
            for line in lines {
                output.push_str(&" ".repeat(marker.len()));
                output.push_str(line);
                output.push('\n');
            }
            number += 1;
        }
        output.push('\n');
        return output;
    }
    if node.tag == "table" {
        fn rows(nodes: &[Node], id: usize, output: &mut Vec<Vec<String>>) {
            if nodes[id].tag == "tr" {
                let cells = nodes[id]
                    .children
                    .iter()
                    .filter(|child| matches!(nodes[**child].tag.as_str(), "td" | "th"))
                    .map(|child| render(nodes, *child).trim().replace('\n', " "))
                    .collect::<Vec<_>>();
                if !cells.is_empty() {
                    output.push(cells);
                }
            } else {
                for child in &nodes[id].children {
                    rows(nodes, *child, output);
                }
            }
        }
        let mut values = Vec::new();
        rows(nodes, id, &mut values);
        let width = values.iter().map(Vec::len).max().unwrap_or(0);
        if width == 0 {
            return String::new();
        }
        let mut output = String::from("\n\n");
        for (at, mut row) in values.into_iter().enumerate() {
            row.resize(width, String::new());
            output.push_str(&format!("| {} |\n", row.join(" | ")));
            if at == 0 {
                output.push_str(&format!("| {} |\n", vec!["---"; width].join(" | ")));
            }
        }
        output.push('\n');
        return output;
    }
    let mut content = String::new();
    for child in &node.children {
        let next = render(nodes, *child);
        if content.ends_with("\n\n") && next.starts_with("\n\n") {
            content.push_str(next.trim_start_matches('\n'));
        } else {
            content.push_str(&next);
        }
    }
    match node.tag.as_str() {
        "strong" | "b" => marked(&content, "**"),
        "em" | "i" => marked(&content, "*"),
        "s" | "strike" | "del" => marked(&content, "~~"),
        "a" => {
            if let Some(href) = &node.href
                && safe_link(href)
            {
                format!(
                    "[{}](<{}>)",
                    content.trim(),
                    href.trim().replace('>', "%3E").replace('<', "%3C")
                )
            } else {
                content
            }
        }
        "br" => "  \n".into(),
        "hr" => "\n\n---\n\n".into(),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => format!(
            "\n\n{} {}\n\n",
            "#".repeat(node.tag.as_bytes()[1] as usize - b'0' as usize),
            content.trim()
        ),
        "blockquote" => format!(
            "\n\n{}\n\n",
            content
                .trim()
                .lines()
                .map(|line| format!("> {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        "p" | "div" | "section" | "article" => format!("\n\n{}\n\n", content.trim()),
        _ => content,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blocks_lists_tables_and_unsafe_content() {
        let md = html_to_markdown("<h2>Title</h2><p>A <b>bold</b> &amp; <s>old</s></p><ul><li>One<ul><li>Two</li></ul></li></ul><table><tr><th>A</th><th>B</th></tr><tr><td>x|y</td><td>z</td></tr></table><script>bad()</script><a href='javascript:bad'>Safe label</a>").unwrap();
        assert!(md.contains("## Title"));
        assert!(md.contains("**bold** & ~~old~~"));
        assert!(md.contains("- One\n  - Two"));
        assert!(md.contains("| x\\|y | z |"));
        assert!(!md.contains("bad"));
        assert!(md.contains("Safe label"));
    }
    #[test]
    fn code_is_literal_and_limits_are_explicit() {
        assert_eq!(
            html_to_markdown("<pre><code>a &lt; b\n```</code></pre>").unwrap(),
            "````\na < b\n```\n````"
        );
        assert!(!safe_link("javaScript:alert(1)"));
        assert!(!safe_link("data:x"));
        assert!(safe_link("../assets/file.pdf"));
        assert!(safe_link("https://example.org"));
        assert!(html_to_markdown(&"<div>".repeat(130)).is_err());
    }
}
