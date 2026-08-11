//! Pure text transforms shared by the bot and the TUI. Having the TUI depend
//! on these instead of re-implementing them means a "this will be tagged
//! #radar #sar" preview it shows before submitting a note uses the exact
//! same regex the bot will apply.

use regex::Regex;
use std::sync::OnceLock;

fn tag_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Compiled once: rebuilding this per call is measurable overhead.
    RE.get_or_init(|| Regex::new(r"#(\w+)").expect("tag regex is valid"))
}

pub fn extract_tags(text: &str) -> Vec<String> {
    let mut tags: Vec<String> = tag_regex()
        .captures_iter(text)
        .map(|c| c[1].to_lowercase())
        .collect();
    tags.sort();
    tags.dedup();
    tags
}

/// Turns a note's first line into a filesystem-safe slug for the vault.
pub fn slugify(text: &str) -> String {
    let slug: String = text
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .take(8)
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        "note".to_string()
    } else {
        slug
    }
}

/// Keeps a caller-supplied filename (from Telegram, the dashboard, or the
/// TUI) from escaping its directory or upsetting object-store key rules.
pub fn sanitize(name: &str) -> String {
    // Everything outside the safe set collapses to a single underscore, then
    // leading/trailing separators and dots go. That turns "../../etc/passwd"
    // into "etc_passwd" rather than a technically-safe but ugly "_.._etc...".
    let mut out = String::new();
    for c in name.chars() {
        match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' => out.push(c),
            _ if !out.ends_with('_') && !out.is_empty() => out.push('_'),
            _ => {}
        }
    }

    let trimmed = out.trim_matches(|c| c == '.' || c == '_' || c == '-');
    if trimmed.is_empty() {
        "file".to_string()
    } else {
        trimmed.chars().take(120).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_lowercased_sorted_and_deduped() {
        assert_eq!(
            extract_tags("a #Radar note about #SAR and #radar again"),
            vec!["radar".to_string(), "sar".to_string()]
        );
    }

    #[test]
    fn text_without_tags_yields_none() {
        assert!(extract_tags("no tags here").is_empty());
    }

    #[test]
    fn slug_uses_first_line_and_bounds_length() {
        assert_eq!(
            slugify("Hello World: a test\nsecond line"),
            "hello-world-a-test"
        );
        assert_eq!(slugify(""), "note");
        assert_eq!(slugify("!!!"), "note");
        assert_eq!(
            slugify("one two three four five six seven eight nine")
                .split('-')
                .count(),
            8
        );
    }

    #[test]
    fn sanitize_strips_path_traversal() {
        assert_eq!(sanitize("../../etc/passwd"), "etc_passwd");
        assert_eq!(sanitize("report final.pdf"), "report_final.pdf");
        assert_eq!(sanitize(""), "file");
        assert_eq!(sanitize("..."), "file");
    }
}
