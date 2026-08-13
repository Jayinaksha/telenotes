//! Turns markdown source into styled `Line`s for the Detail view. Not a full
//! renderer -- no tables, no images, no LaTeX (a plain terminal can't draw
//! math without a graphics protocol like kitty/sixel, which isn't portable
//! across the platforms this TUI targets) -- just enough styling (headings,
//! emphasis, code, lists, blockquotes) to make an uploaded `.md` file
//! readable instead of dumping raw source, matching what the web dashboard's
//! "Render" toggle does for notes.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Mirrors `isMarkdownDocument` in the web dashboard's `render.js`: browsers
/// (and, here, whatever set the upload's mime type) often leave `.md`
/// uploads with no mime type at all, so the file extension is as reliable a
/// signal as the declared one.
pub fn is_markdown_document(file_name: Option<&str>, mime_type: Option<&str>) -> bool {
    if mime_type == Some("text/markdown") {
        return true;
    }
    file_name
        .map(|name| {
            let lower = name.to_ascii_lowercase();
            lower.ends_with(".md") || lower.ends_with(".markdown")
        })
        .unwrap_or(false)
}

pub fn render(text: &str) -> Vec<Line<'static>> {
    let parser = Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    // Innermost active style is last; inline tags push/pop their own entry so
    // nesting (e.g. **bold _and italic_**) composes correctly.
    let mut styles: Vec<Style> = vec![Style::default()];
    let mut in_code_block = false;
    // One entry per open list; `Some(n)` is an ordered list's next number,
    // `None` is a bullet list. Depth of the stack is the indent level.
    let mut list_stack: Vec<Option<u64>> = Vec::new();

    let flush = |lines: &mut Vec<Line<'static>>, current: &mut Vec<Span<'static>>| {
        if !current.is_empty() {
            lines.push(Line::from(std::mem::take(current)));
        }
    };

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    flush(&mut lines, &mut current);
                    let color = match level {
                        HeadingLevel::H1 => Color::Cyan,
                        HeadingLevel::H2 => Color::Blue,
                        _ => Color::Magenta,
                    };
                    styles.push(Style::default().fg(color).add_modifier(Modifier::BOLD));
                }
                Tag::Emphasis => {
                    styles.push(
                        styles
                            .last()
                            .copied()
                            .unwrap_or_default()
                            .add_modifier(Modifier::ITALIC),
                    );
                }
                Tag::Strong => {
                    styles.push(
                        styles
                            .last()
                            .copied()
                            .unwrap_or_default()
                            .add_modifier(Modifier::BOLD),
                    );
                }
                Tag::Strikethrough => {
                    styles.push(
                        styles
                            .last()
                            .copied()
                            .unwrap_or_default()
                            .add_modifier(Modifier::CROSSED_OUT),
                    );
                }
                Tag::Link { .. } => {
                    styles.push(
                        styles
                            .last()
                            .copied()
                            .unwrap_or_default()
                            .add_modifier(Modifier::UNDERLINED),
                    );
                }
                Tag::BlockQuote(_) => {
                    flush(&mut lines, &mut current);
                    styles.push(
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    );
                    current.push(Span::styled("> ", *styles.last().unwrap()));
                }
                Tag::CodeBlock(_) => {
                    flush(&mut lines, &mut current);
                    in_code_block = true;
                    styles.push(Style::default().fg(Color::Yellow));
                }
                Tag::List(start) => list_stack.push(start),
                Tag::Item => {
                    flush(&mut lines, &mut current);
                    let indent = "  ".repeat(list_stack.len().saturating_sub(1));
                    let marker = match list_stack.last_mut() {
                        Some(Some(n)) => {
                            let s = format!("{indent}{n}. ");
                            *n += 1;
                            s
                        }
                        _ => format!("{indent}- "),
                    };
                    current.push(Span::raw(marker));
                }
                _ => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Heading(_) => {
                    flush(&mut lines, &mut current);
                    styles.pop();
                    lines.push(Line::default());
                }
                TagEnd::Paragraph | TagEnd::Item => flush(&mut lines, &mut current),
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                    styles.pop();
                }
                TagEnd::BlockQuote(_) => {
                    flush(&mut lines, &mut current);
                    styles.pop();
                }
                TagEnd::CodeBlock => {
                    flush(&mut lines, &mut current);
                    styles.pop();
                    in_code_block = false;
                }
                TagEnd::List(_) => {
                    list_stack.pop();
                }
                _ => {}
            },
            Event::Text(text) => {
                let style = *styles.last().unwrap();
                if in_code_block {
                    let mut parts = text.split('\n');
                    if let Some(first) = parts.next() {
                        current.push(Span::styled(first.to_string(), style));
                    }
                    for rest in parts {
                        flush(&mut lines, &mut current);
                        current.push(Span::styled(rest.to_string(), style));
                    }
                } else {
                    current.push(Span::styled(text.to_string(), style));
                }
            }
            Event::Code(code) => {
                current.push(Span::styled(
                    code.to_string(),
                    Style::default().fg(Color::Yellow),
                ));
            }
            Event::SoftBreak => current.push(Span::raw(" ")),
            Event::HardBreak => flush(&mut lines, &mut current),
            Event::Rule => {
                flush(&mut lines, &mut current);
                lines.push(Line::styled("---", Style::default().fg(Color::DarkGray)));
            }
            _ => {}
        }
    }
    flush(&mut lines, &mut current);
    lines
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
        let sample = "# Title\n\nSome **bold** and _italic_ text, plus `inline code`.\n\n\
             - one\n- two\n  - nested\n\n1. first\n2. second\n\n> a quote\n\n\
             ```\nfn code() {}\n```\n\n[a link](https://example.com)\n";
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
    }

    #[test]
    fn is_markdown_document_falls_back_to_extension() {
        assert!(is_markdown_document(Some("notes.md"), None));
        assert!(is_markdown_document(Some("NOTES.MARKDOWN"), None));
        assert!(is_markdown_document(None, Some("text/markdown")));
        assert!(!is_markdown_document(
            Some("report.pdf"),
            Some("application/pdf")
        ));
        assert!(!is_markdown_document(None, None));
    }
}
