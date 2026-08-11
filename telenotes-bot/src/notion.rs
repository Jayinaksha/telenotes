//! One-way mirror into a Notion database.
//!
//! This is the integration point, wired but dormant: nothing happens unless
//! both `NOTION_TOKEN` and `NOTION_DATABASE_ID` are set, so you can turn it on
//! later without touching code (see SETUP.md, "Notion").
//!
//! Writes flow one way -- Postgres to Notion -- and edits you make in Notion
//! itself are never read back; Postgres and the vault stay the sources of
//! truth. But an edit made through telenotes (Telegram, dashboard, or the
//! TUI) *is* propagated to the mirrored page, via `update_page`, so the
//! Notion copy doesn't silently drift out of date every time you fix a typo.
//!
//! Mirroring is best-effort throughout: a Notion outage logs a warning and
//! never fails the underlying capture/edit/delete, because losing the note
//! would be much worse than losing the mirror.

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

    /// Creates one page per captured item, returning its page id so a later
    /// edit or delete can find it again (callers store this on the `items`
    /// row as `notion_page_id`).
    ///
    /// The target database needs these properties, spelled exactly:
    ///   Name (title), Kind (select), Tags (multi-select), ID (rich text).
    pub async fn create_page(&self, page: NotionPage<'_>) -> Result<String> {
        let body = json!({
            "parent": { "database_id": self.database_id },
            "properties": properties_json(&page),
            "children": page.body.map(paragraph_blocks).unwrap_or_default(),
        });

        let resp = self
            .request(
                reqwest::Method::POST,
                "https://api.notion.com/v1/pages".to_string(),
            )
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("Notion page creation failed ({status}): {text}");
        }
        let value: Value = resp.json().await?;
        value
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("Notion page creation response had no id"))
    }

    /// Updates a previously mirrored page's properties (title/kind/tags),
    /// and, when `sync_body` is set, replaces its content blocks with
    /// `page.body`. Notion has no "replace" for block children, so a body
    /// sync archives every existing block before appending the new ones.
    pub async fn update_page(
        &self,
        page_id: &str,
        page: NotionPage<'_>,
        sync_body: bool,
    ) -> Result<()> {
        let resp = self
            .request(
                reqwest::Method::PATCH,
                format!("https://api.notion.com/v1/pages/{page_id}"),
            )
            .json(&json!({ "properties": properties_json(&page) }))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("Notion page property update failed ({status}): {text}");
        }

        if sync_body {
            self.replace_children(page_id, page.body).await?;
        }
        Ok(())
    }

    /// Archives (soft-deletes) a mirrored page. Notion has no hard delete via
    /// the public API; archiving is what "moved to trash" means there.
    pub async fn archive_page(&self, page_id: &str) -> Result<()> {
        let resp = self
            .request(
                reqwest::Method::PATCH,
                format!("https://api.notion.com/v1/pages/{page_id}"),
            )
            .json(&json!({ "archived": true }))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("Notion page archive failed ({status}): {text}");
        }
        Ok(())
    }

    async fn replace_children(&self, page_id: &str, body: Option<&str>) -> Result<()> {
        let resp = self
            .request(
                reqwest::Method::GET,
                format!("https://api.notion.com/v1/blocks/{page_id}/children"),
            )
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("Notion block listing failed ({status}): {text}");
        }
        let listing: Value = resp.json().await?;
        let block_ids: Vec<String> = listing
            .get("results")
            .and_then(|r| r.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|b| b.get("id").and_then(|v| v.as_str()).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        for block_id in block_ids {
            let resp = self
                .request(
                    reqwest::Method::PATCH,
                    format!("https://api.notion.com/v1/blocks/{block_id}"),
                )
                .json(&json!({ "archived": true }))
                .send()
                .await?;
            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                bail!("Notion block archive failed ({status}): {text}");
            }
        }

        let children = body.map(paragraph_blocks).unwrap_or_default();
        if !children.is_empty() {
            let resp = self
                .request(
                    reqwest::Method::PATCH,
                    format!("https://api.notion.com/v1/blocks/{page_id}/children"),
                )
                .json(&json!({ "children": children }))
                .send()
                .await?;
            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                bail!("Notion block append failed ({status}): {text}");
            }
        }
        Ok(())
    }

    fn request(&self, method: reqwest::Method, url: String) -> reqwest::RequestBuilder {
        self.http
            .request(method, url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Notion-Version", NOTION_VERSION)
    }
}

fn properties_json(page: &NotionPage<'_>) -> Value {
    let mut properties = json!({
        "Name":  { "title": [{ "text": { "content": truncate(page.title, 200) } }] },
        "Kind":  { "select": { "name": page.kind } },
        "ID":    { "rich_text": [{ "text": { "content": page.short_id } }] },
    });
    properties["Tags"] = json!({
        "multi_select": page.tags.iter().map(|t| json!({ "name": t })).collect::<Vec<_>>()
    });
    properties
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
