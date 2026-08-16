//! Converts a note's markdown source into a string safe to send with
//! `ParseMode::Html`, so `/get` shows bold/italic/links/code/lists as
//! actual Telegram formatting instead of inert `*`/`_`/`` ` `` characters.
//!
//! Telegram's HTML subset is narrower than real HTML: no headings, no
//! `<ul>`/`<li>`. Headings become bold; lists flatten to `-`/`1.`-prefixed
//! lines. Inline math (`$..$`/`$$..$$`, the same convention
//! `web/markdown.js` treats as LaTeX) stays as raw source inside `<code>`
//! -- Telegram has no native math rendering at all, so rendering it
//! visually would mean a whole new image-generation pipeline this
//! codebase doesn't have.

use pulldown_cmark::{Options, Parser, Tag, TagEnd};
use teloxide::utils::html;

pub fn render(text: &str) -> String {
    let parser = Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH | Options::ENABLE_MATH);

    // Inline tags (Strong/Emphasis/Strikethrough/Heading/BlockQuote/
    // CodeBlock/Link) can nest -- `**bold _and italic_**`, a link whose
    // text is itself bold -- so each one gets an isolated buffer on this
    // stack; on End, it's popped, wrapped with the matching `html::`
    // helper, and appended to what's now the top. That composes correctly
    // without double-wrapping or double-escaping.
    let mut bufs: Vec<String> = vec![String::new()];
    let mut link_urls: Vec<String> = Vec::new();
    let mut list_stack: Vec<Option<u64>> = Vec::new();
    let mut in_code_block = false;

    macro_rules! push {
        ($s:expr) => {
            bufs.last_mut().unwrap().push_str(&$s)
        };
    }

    for event in parser {
        match event {
            pulldown_cmark::Event::Start(tag) => match tag {
                Tag::Heading { .. }
                | Tag::Strong
                | Tag::Emphasis
                | Tag::Strikethrough
                | Tag::BlockQuote(_) => bufs.push(String::new()),
                Tag::CodeBlock(_) => {
                    in_code_block = true;
                    bufs.push(String::new());
                }
                Tag::Link { dest_url, .. } => {
                    link_urls.push(dest_url.to_string());
                    bufs.push(String::new());
                }
                Tag::List(start) => list_stack.push(start),
                Tag::Item => {
                    let indent = "  ".repeat(list_stack.len().saturating_sub(1));
                    let marker = match list_stack.last_mut() {
                        Some(Some(n)) => {
                            let s = format!("{indent}{n}. ");
                            *n += 1;
                            s
                        }
                        _ => format!("{indent}- "),
                    };
                    push!(marker);
                }
                _ => {}
            },
            pulldown_cmark::Event::End(tag_end) => match tag_end {
                TagEnd::Heading(_) => {
                    let inner = bufs.pop().unwrap();
                    push!(html::bold(&inner));
                    push!("\n\n");
                }
                TagEnd::Paragraph => push!("\n\n"),
                TagEnd::Strong => {
                    let inner = bufs.pop().unwrap();
                    push!(html::bold(&inner));
                }
                TagEnd::Emphasis => {
                    let inner = bufs.pop().unwrap();
                    push!(html::italic(&inner));
                }
                TagEnd::Strikethrough => {
                    let inner = bufs.pop().unwrap();
                    push!(html::strike(&inner));
                }
                TagEnd::Link => {
                    let inner = bufs.pop().unwrap();
                    let url = link_urls.pop().unwrap_or_default();
                    // Not html::link(): it escapes its `text` argument
                    // internally, which would double-escape (or mangle)
                    // any nested formatting already produced above, e.g.
                    // a bold link.
                    push!(format!("<a href=\"{}\">{inner}</a>", html::escape(&url)));
                }
                TagEnd::CodeBlock => {
                    // Fenced/indented code blocks aren't parsed for inline
                    // markdown by CommonMark, so this buffer holds raw,
                    // unescaped text -- code_block() escapes it once here.
                    let inner = bufs.pop().unwrap();
                    push!(html::code_block(&inner));
                    push!("\n\n");
                    in_code_block = false;
                }
                TagEnd::BlockQuote(_) => {
                    let inner = bufs.pop().unwrap();
                    push!(html::blockquote(inner.trim()));
                    push!("\n\n");
                }
                TagEnd::Item => push!("\n"),
                TagEnd::List(_) => {
                    list_stack.pop();
                    push!("\n");
                }
                _ => {}
            },
            pulldown_cmark::Event::Text(text) => {
                if in_code_block {
                    push!(text);
                } else {
                    push!(html::escape(&text));
                }
            }
            // Backtick code spans are literal text by spec, no nested
            // markdown -- code_inline() escapes internally, so this can't
            // double-escape.
            pulldown_cmark::Event::Code(code) => push!(html::code_inline(&code)),
            pulldown_cmark::Event::InlineMath(tex) | pulldown_cmark::Event::DisplayMath(tex) => {
                push!(html::code_inline(&tex));
            }
            pulldown_cmark::Event::SoftBreak => push!(" "),
            pulldown_cmark::Event::HardBreak => push!("\n"),
            pulldown_cmark::Event::Rule => push!("\n\n"),
            _ => {}
        }
    }

    bufs.pop().unwrap().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bold_italic_and_strikethrough() {
        assert_eq!(render("**bold**"), "<b>bold</b>");
        assert_eq!(render("_italic_"), "<i>italic</i>");
        assert_eq!(render("~~gone~~"), "<s>gone</s>");
    }

    #[test]
    fn formats_nested_emphasis() {
        assert_eq!(
            render("**bold _and italic_**"),
            "<b>bold <i>and italic</i></b>"
        );
    }

    #[test]
    fn formats_inline_and_block_code_without_double_escaping() {
        assert_eq!(render("`a < b`"), "<code>a &lt; b</code>");
        assert_eq!(render("```\na < b\n```"), "<pre>a &lt; b\n</pre>");
    }

    #[test]
    fn formats_links_including_nested_bold_without_double_escaping() {
        assert_eq!(
            render("[a link](https://example.com?a=1&b=2)"),
            "<a href=\"https://example.com?a=1&amp;b=2\">a link</a>"
        );
        assert_eq!(
            render("[**bold link**](https://example.com)"),
            "<a href=\"https://example.com\"><b>bold link</b></a>"
        );
    }

    #[test]
    fn formats_lists_as_prefixed_lines() {
        assert_eq!(render("- one\n- two"), "- one\n- two");
        assert_eq!(render("1. first\n2. second"), "1. first\n2. second");
    }

    #[test]
    fn formats_blockquote() {
        assert_eq!(render("> a quote"), "<blockquote>a quote</blockquote>");
    }

    #[test]
    fn math_stays_as_escaped_source_in_a_code_span() {
        assert_eq!(render("$x^2$"), "<code>x^2</code>");
        assert_eq!(render("$$x < y$$"), "<code>x &lt; y</code>");
    }

    #[test]
    fn heading_becomes_bold() {
        assert_eq!(render("# Title"), "<b>Title</b>");
    }

    #[test]
    fn plain_text_is_escaped() {
        assert_eq!(render("a < b & c > d"), "a &lt; b &amp; c &gt; d");
    }
}
