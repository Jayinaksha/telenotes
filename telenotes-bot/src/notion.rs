//! One-way mirror into a Notion database.
//!
//! This is the integration point, wired but dormant: nothing happens unless
//! both `NOTION_TOKEN` and `NOTION_DATABASE_ID` are set, so you can turn it on
//! later without touching code (see SETUP.md, "Notion").
//!
//! Deliberately one-way. Postgres stays the source of truth for search and
//! the vault stays the source of truth for bytes; Notion is a reading surface.
//! That means there is no sync conflict to resolve -- edits you make in Notion
//! simply aren't read back.
//!
//! Mirroring is best-effort: a Notion outage logs a warning and never fails
//! the capture, because losing the note would be much worse than losing the
//! mirror.

use anyhow::{bail, Result};
use reqwest::Client;
use serde_json::{json, Value};

const NOTION_VERSION: &str = "2022-06-28";
/// Notion rejects rich-text segments longer than 2000 characters.
const MAX_TEXT_CHUNK: usize = 1900;

#[derive(Clone)]
pub struct NotionClient {
    http: Client,
    token: String,
    database_id: String,
}

/// What to mirror. Kept separate from `Item` so the mirror doesn't depend on
/// the database row shape.
pub struct NotionPage<'a> {
    pub title: &'a str,
    pub kind: &'a str,
    pub body: Option<&'a str>,
    pub tags: &'a [String],
    pub short_id: &'a str,
}

impl NotionClient {
    pub fn new(http: Client, token: String, database_id: String) -> Self {
        Self {
            http,
            token,
            database_id,
        }
    }

    /// Creates one page per captured item.
    ///
    /// The target database needs these properties, spelled exactly:
    ///   Name (title), Kind (select), Tags (multi-select), ID (rich text).
    pub async fn create_page(&self, page: NotionPage<'_>) -> Result<()> {
        let mut properties = json!({
            "Name":  { "title": [{ "text": { "content": truncate(page.title, 200) } }] },
            "Kind":  { "select": { "name": page.kind } },
            "ID":    { "rich_text": [{ "text": { "content": page.short_id } }] },
        });
        if !page.tags.is_empty() {
            properties["Tags"] = json!({
                "multi_select": page.tags.iter().map(|t| json!({ "name": t })).collect::<Vec<_>>()
            });
        }

        let body = json!({
            "parent": { "database_id": self.database_id },
            "properties": properties,
            "children": page.body.map(paragraph_blocks).unwrap_or_default(),
        });

        let resp = self
            .http
            .post("https://api.notion.com/v1/pages")
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Notion-Version", NOTION_VERSION)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("Notion page creation failed ({status}): {text}");
        }
        Ok(())
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

/// Splits body text into paragraph blocks under Notion's per-segment limit.
fn paragraph_blocks(text: &str) -> Vec<Value> {
    let chars: Vec<char> = text.chars().collect();
    chars
        .chunks(MAX_TEXT_CHUNK)
        .map(|chunk| {
            let content: String = chunk.iter().collect();
            json!({
                "object": "block",
                "type": "paragraph",
                "paragraph": {
                    "rich_text": [{ "type": "text", "text": { "content": content } }]
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_bodies_split_into_multiple_blocks() {
        let text = "x".repeat(MAX_TEXT_CHUNK * 2 + 5);
        let blocks = paragraph_blocks(&text);
        assert_eq!(blocks.len(), 3);
    }

    #[test]
    fn truncate_counts_characters_not_bytes() {
        // Naive byte slicing would panic here.
        assert_eq!(truncate("日本語テキスト", 3), "日本語");
    }
}
