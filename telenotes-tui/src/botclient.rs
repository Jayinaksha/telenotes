//! Talks to the bot's `/api/*` write endpoints and the `/d/{token}` download
//! redirect -- the same token-capability surface the dashboard uses, hit
//! here with `reqwest` instead of `fetch()`. CORS is a browser-only
//! mechanism, so none of this needs an origin exemption on the bot side.

use anyhow::{bail, Context, Result};
use serde_json::json;
use telenotes_core::Item;
use uuid::Uuid;

pub struct BotClient {
    http: reqwest::Client,
    base_url: String,
}

impl BotClient {
    pub fn new(base_url: String) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent("telenotes-tui")
                .build()?,
            base_url,
        })
    }

    pub async fn create_note(&self, token: &str, text: &str) -> Result<Item> {
        let resp = self
            .http
            .post(format!("{}/api/notes", self.base_url))
            .json(&json!({ "token": token, "text": text }))
            .send()
            .await?;
        parse(resp).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn edit_item(
        &self,
        token: &str,
        item_id: Uuid,
        content_text: Option<&str>,
        tags: Option<&[String]>,
        file_name: Option<&str>,
        mime_type: Option<&str>,
    ) -> Result<Item> {
        let resp = self
            .http
            .post(format!("{}/api/items/{item_id}/edit", self.base_url))
            .json(&json!({
                "token": token,
                "content_text": content_text,
                "tags": tags,
                "file_name": file_name,
                "mime_type": mime_type,
            }))
            .send()
            .await?;
        parse(resp).await
    }

    pub async fn delete_item(&self, token: &str, item_id: Uuid) -> Result<Item> {
        let resp = self
            .http
            .delete(format!("{}/api/items/{item_id}", self.base_url))
            .query(&[("token", token)])
            .send()
            .await?;
        parse(resp).await
    }

    pub async fn upload_file(
        &self,
        token: &str,
        kind: &str,
        bytes: Vec<u8>,
        file_name: &str,
        caption: Option<&str>,
    ) -> Result<Item> {
        let mut form = reqwest::multipart::Form::new()
            .text("token", token.to_string())
            .text("kind", kind.to_string())
            .part(
                "file",
                reqwest::multipart::Part::bytes(bytes).file_name(file_name.to_string()),
            );
        if let Some(caption) = caption {
            form = form.text("caption", caption.to_string());
        }
        let resp = self
            .http
            .post(format!("{}/api/files", self.base_url))
            .multipart(form)
            .send()
            .await?;
        parse(resp).await
    }

    /// Follows `/d/{token}` -- works identically for the R2-presigned
    /// redirect and the GitHub-streamed case, since `reqwest` follows
    /// redirects by default.
    pub async fn download(&self, token: &str) -> Result<Vec<u8>> {
        let resp = self
            .http
            .get(format!("{}/d/{token}", self.base_url))
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!("download failed: {}", resp.text().await.unwrap_or_default());
        }
        Ok(resp.bytes().await?.to_vec())
    }
}

async fn parse(resp: reqwest::Response) -> Result<Item> {
    if !resp.status().is_success() {
        let status = resp.status();
        bail!(
            "request failed ({status}): {}",
            resp.text().await.unwrap_or_default()
        );
    }
    resp.json().await.context("unexpected response shape")
}
