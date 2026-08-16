//! Turns HTML source into styled `Line`s for the Detail view, the same
//! scope decision as `markdown.rs`: enough styling (headings, emphasis,
//! code, lists, blockquotes, links) to make an uploaded HTML file readable
//! as terminal text, not a browser -- no tables, no images, no CSS layout,
//! no LaTeX (same reasoning as markdown: no math glyphs without a graphics
//! protocol this TUI can't portably assume).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use tl::{HTMLTag, Node, NodeHandle, Parser};

/// Mirrors `is_markdown_document`: mime type first, file extension as the
/// fallback browsers so often force us to.
pub fn is_html_document(file_name: Option<&str>, mime_type: Option<&str>) -> bool {
    if mime_type == Some("text/html") {
        return true;
    }
    file_name
        .map(|name| {
            let lower = name.to_ascii_lowercase();
            lower.ends_with(".html") || lower.ends_with(".htm")
        })
        .unwrap_or(false)
}

struct Ctx {
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    styles: Vec<Style>,
    // One entry per open <ul>/<ol>; `Some(n)` is an ordered list's next
    // number, `None` is a bullet list -- same shape as markdown.rs's
    // list_stack.
    list_stack: Vec<Option<u64>>,
    // <pre> preserves whitespace verbatim; everywhere else HTML collapses
    // runs of whitespace to a single space, same as a browser would.
    in_pre: bool,
}

impl Ctx {
    fn flush(&mut self) {
        if !self.current.is_empty() {
            self.lines
                .push(Line::from(std::mem::take(&mut self.current)));
        }
    }

    fn style(&self) -> Style {
        *self.styles.last().unwrap()
    }

    fn push_text(&mut self, text: &str) {
        let style = self.style();
        if self.in_pre {
            let mut parts = text.split('\n');
            if let Some(first) = parts.next() {
                if !first.is_empty() {
                    self.current.push(Span::styled(first.to_string(), style));
                }
            }
            for rest in parts {
                self.flush();
                self.current.push(Span::styled(rest.to_string(), style));
            }
        } else {
            let collapsed = collapse_whitespace(text);
            if !collapsed.is_empty() {
                self.current.push(Span::styled(collapsed, style));
            }
        }
    }
}

/// Collapses runs of whitespace (including newlines from pretty-printed
/// source) to a single space, and trims the ends -- standard HTML
/// whitespace handling outside `<pre>`.
fn collapse_whitespace(s: &str) -> String {
    let mut out = String::new();
    let mut prev_space = true; // starts true so leading whitespace is trimmed
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

pub fn render(html: &str) -> Vec<Line<'static>> {
    let Ok(dom) = tl::parse(html, tl::ParserOptions::default()) else {
        return vec![Line::from("(couldn't parse this file as HTML)")];
    };
    let parser = dom.parser();

    let mut ctx = Ctx {
        lines: Vec::new(),
        current: Vec::new(),
        styles: vec![Style::default()],
        list_stack: Vec::new(),
        in_pre: false,
    };

    for handle in dom.children() {
        walk(*handle, parser, &mut ctx);
    }
    ctx.flush();
    ctx.lines
}

fn walk(handle: NodeHandle, parser: &Parser, ctx: &mut Ctx) {
    let Some(node) = handle.get(parser) else {
        return;
    };
    match node {
        Node::Comment(_) => {}
        Node::Raw(text) => ctx.push_text(&text.as_utf8_str()),
        Node::Tag(tag) => walk_tag(tag.name().as_utf8_str().as_ref(), tag, parser, ctx),
    }
}

fn walk_children(tag: &HTMLTag, parser: &Parser, ctx: &mut Ctx) {
    for handle in tag.children().top().iter() {
        walk(*handle, parser, ctx);
    }
}

fn walk_tag(name: &str, tag: &HTMLTag, parser: &Parser, ctx: &mut Ctx) {
    match name {
        // No visible text, and no reason to preview raw CSS/JS as noise in
        // a terminal -- unlike the web path there's no execution risk
        // either way, this is purely about readability.
        "script" | "style" | "head" | "title" => {}

        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            ctx.flush();
            let color = match name {
                "h1" => Color::Cyan,
                "h2" => Color::Blue,
                _ => Color::Magenta,
            };
            ctx.styles
                .push(Style::default().fg(color).add_modifier(Modifier::BOLD));
            walk_children(tag, parser, ctx);
            ctx.flush();
            ctx.styles.pop();
            ctx.lines.push(Line::default());
        }
        "b" | "strong" => {
            ctx.styles.push(ctx.style().add_modifier(Modifier::BOLD));
            walk_children(tag, parser, ctx);
            ctx.styles.pop();
        }
        "i" | "em" => {
            ctx.styles.push(ctx.style().add_modifier(Modifier::ITALIC));
            walk_children(tag, parser, ctx);
            ctx.styles.pop();
        }
        "s" | "strike" | "del" => {
            ctx.styles
                .push(ctx.style().add_modifier(Modifier::CROSSED_OUT));
            walk_children(tag, parser, ctx);
            ctx.styles.pop();
        }
        "a" => {
            ctx.styles
                .push(ctx.style().add_modifier(Modifier::UNDERLINED));
            walk_children(tag, parser, ctx);
            ctx.styles.pop();
        }
        "code" => {
            ctx.styles.push(Style::default().fg(Color::Yellow));
            walk_children(tag, parser, ctx);
            ctx.styles.pop();
        }
        "pre" => {
            ctx.flush();
            let was_in_pre = ctx.in_pre;
            ctx.in_pre = true;
            ctx.styles.push(Style::default().fg(Color::Yellow));
            walk_children(tag, parser, ctx);
            ctx.flush();
            ctx.styles.pop();
            ctx.in_pre = was_in_pre;
        }
        "blockquote" => {
            ctx.flush();
            ctx.styles.push(
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            );
            ctx.current.push(Span::styled("> ", ctx.style()));
            walk_children(tag, parser, ctx);
            ctx.flush();
            ctx.styles.pop();
        }
        "ul" => {
            ctx.list_stack.push(None);
            walk_children(tag, parser, ctx);
            ctx.list_stack.pop();
        }
        "ol" => {
            ctx.list_stack.push(Some(1));
            walk_children(tag, parser, ctx);
            ctx.list_stack.pop();
        }
        "li" => {
            ctx.flush();
            let indent = "  ".repeat(ctx.list_stack.len().saturating_sub(1));
            let marker = match ctx.list_stack.last_mut() {
                Some(Some(n)) => {
                    let s = format!("{indent}{n}. ");
                    *n += 1;
                    s
                }
                _ => format!("{indent}- "),
            };
            ctx.current.push(Span::raw(marker));
            walk_children(tag, parser, ctx);
            ctx.flush();
        }
        "br" => ctx.flush(),
        "p" | "div" => {
            ctx.flush();
            walk_children(tag, parser, ctx);
            ctx.flush();
        }
        // Unrecognized tag: descend into its content with no style change
        // rather than dropping it -- most tags (span, section, article,
        // ...) are just grouping, not formatting.
        _ => walk_children(tag, parser, ctx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn renders_headings_lists_and_code_without_panicking() {
        let sample = r#"
            <html><head><style>body { color: red; }</style></head>
            <body>
              <h1>Title</h1>
              <p>Some <b>bold</b> and <i>italic</i> text, plus <code>inline code</code>.</p>
              <ul><li>one</li><li>two<ul><li>nested</li></ul></li></ul>
              <ol><li>first</li><li>second</li></ol>
              <blockquote>a quote</blockquote>
              <pre>fn code() {}</pre>
              <a href="https://example.com">a link</a>
              <script>alert("should not appear")</script>
            </body></html>
        "#;
        let lines = plain(&render(sample));
        assert!(lines.iter().any(|l| l.contains("Title")));
        assert!(lines
            .iter()
            .any(|l| l.contains("bold") && l.contains("italic")));
        assert!(lines.iter().any(|l| l.contains("- one")));
        assert!(lines.iter().any(|l| l.contains("1. first")));
        assert!(lines.iter().any(|l| l.contains("2. second")));
        assert!(lines.iter().any(|l| l.contains("> a quote")));
        assert!(lines.iter().any(|l| l.contains("fn code()")));
        assert!(lines.iter().any(|l| l.contains("a link")));
        assert!(!lines.iter().any(|l| l.contains("should not appear")));
        assert!(!lines.iter().any(|l| l.contains("color: red")));
    }

    #[test]
    fn is_html_document_falls_back_to_extension() {
        assert!(is_html_document(Some("page.html"), None));
        assert!(is_html_document(Some("PAGE.HTM"), None));
        assert!(is_html_document(None, Some("text/html")));
        assert!(!is_html_document(Some("notes.md"), Some("text/markdown")));
        assert!(!is_html_document(None, None));
    }
}
