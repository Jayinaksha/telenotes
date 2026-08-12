//! Chunking + embedding for semantic search. Wired but dormant like Notion --
//! nothing happens until `VOYAGE_API_KEY` is set (see SETUP.md).
//!
//! Chunking, embedding, and storage all run best-effort in a spawned
//! background task after a note/file is saved -- same philosophy as the
//! Notion mirror, just async instead of inline, since embedding a large PDF
//! means several round trips to Voyage and shouldn't delay the "Saved"
//! confirmation.

use anyhow::{bail, Result};
use pgvector::Vector;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::db;
use crate::AppState;

const MODEL: &str = "voyage-4";
const DIMENSIONS: u32 = 1024;
/// ~800 tokens at ~4 chars/token, with ~12% overlap so a fact split across a
/// chunk boundary is still fully covered by at least one chunk.
const CHUNK_CHARS: usize = 3200;
const CHUNK_OVERLAP: usize = 400;

#[derive(Clone)]
pub struct VoyageClient {
    http: Client,
    api_key: String,
}

#[derive(Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

impl VoyageClient {
    pub fn new(http: Client, api_key: String) -> Self {
        Self { http, api_key }
    }

    /// `input_type` is "document" for content being indexed, "query" for a
    /// search string -- Voyage's models are trained asymmetrically, so
    /// tagging which side of the search each text is on measurably improves
    /// retrieval quality.
    async fn embed(&self, texts: &[String], input_type: &str) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let resp = self
            .http
            .post("https://api.voyageai.com/v1/embeddings")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&json!({
                "input": texts,
                "model": MODEL,
                "input_type": input_type,
                "output_dimension": DIMENSIONS,
            }))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("Voyage embed request failed ({status}): {text}");
        }
        let body: EmbedResponse = resp.json().await?;
        Ok(body.embeddings)
    }
}

/// Splits `text` into ~CHUNK_CHARS pieces on whitespace boundaries where
/// possible, with CHUNK_OVERLAP of trailing context carried into the next
/// chunk. Notes are usually one chunk; PDFs are usually many.
pub fn chunk_text(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= CHUNK_CHARS {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = (start + CHUNK_CHARS).min(chars.len());
        if end < chars.len() {
            if let Some(back) = chars[start..end].iter().rposition(|c| c.is_whitespace()) {
                if back > CHUNK_CHARS / 2 {
                    end = start + back;
                }
            }
        }
        let chunk: String = chars[start..end].iter().collect();
        let trimmed = chunk.trim();
        if !trimmed.is_empty() {
            chunks.push(trimmed.to_string());
        }
        if end >= chars.len() {
            break;
        }
        start = end.saturating_sub(CHUNK_OVERLAP);
    }
    chunks
}

/// Chunks and embeds `text`, replacing whatever chunks this item already
/// had. Best-effort throughout -- same discipline as the Notion mirror: logs
/// a warning and leaves the item itself untouched on any failure.
pub async fn embed_item(state: &AppState, item_id: Uuid, user_id: Uuid, text: &str) {
    let Some(voyage) = &state.voyage else {
        return;
    };

    let chunks = chunk_text(text);
    if chunks.is_empty() {
        return;
    }

    let embeddings = match voyage.embed(&chunks, "document").await {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("embedding failed for item {item_id}: {e:?}");
            return;
        }
    };
    if embeddings.len() != chunks.len() {
        tracing::warn!(
            "embedding count mismatch for item {item_id}: {} chunks, {} embeddings",
            chunks.len(),
            embeddings.len()
        );
        return;
    }

    let rows: Vec<(String, Vector)> = chunks
        .into_iter()
        .zip(embeddings.into_iter().map(Vector::from))
        .collect();

    if let Err(e) = db::replace_chunks(&state.db, item_id, user_id, &rows).await {
        tracing::warn!("storing chunks failed for item {item_id}: {e:?}");
    }
}

/// Embeds a single query string for semantic search, tagged as the query
/// side of Voyage's asymmetric embedding pair.
pub async fn embed_query(state: &AppState, query: &str) -> Result<Vector> {
    let Some(voyage) = &state.voyage else {
        bail!("semantic search is not configured (VOYAGE_API_KEY unset)");
    };
    let mut embeddings = voyage.embed(&[query.to_string()], "query").await?;
    embeddings
        .pop()
        .map(Vector::from)
        .ok_or_else(|| anyhow::anyhow!("Voyage returned no embedding for the query"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_a_single_chunk() {
        assert_eq!(chunk_text("hello world").len(), 1);
    }

    #[test]
    fn empty_text_has_no_chunks() {
        assert!(chunk_text("   ").is_empty());
    }

    #[test]
    fn long_text_splits_into_overlapping_chunks() {
        let text = "word ".repeat(2000); // ~10000 chars
        let chunks = chunk_text(&text);
        assert!(chunks.len() > 1);
        for c in &chunks {
            assert!(!c.is_empty());
        }
    }
}
