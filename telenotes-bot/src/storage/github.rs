//! GitHub Contents API backend.
//!
//! Used for text notes (which become readable, diffable markdown in the
//! vault repo) and, when R2 isn't configured, for binary files too.
//!
//! Be aware of what a git-backed store means for binaries: every upload is a
//! commit, and deleting a file only removes it from the *tip* -- the blob
//! stays in history forever and keeps counting against the repo size. That's
//! the main reason `Vault` prefers R2 for media when it's available.

use anyhow::{bail, Result};
use base64::Engine;
use reqwest::Client;
use serde_json::json;

/// GitHub's Contents API accepts a base64 body; it degrades badly well before
/// git's hard 100MB blob limit, so we stop at 45MB with a clear error rather
/// than letting the request fail opaquely.
const MAX_FILE_BYTES: usize = 45 * 1024 * 1024;

#[derive(Clone)]
pub struct GitHubStore {
    http: Client,
    token: String,
    owner: String,
    repo: String,
    branch: String,
}

impl GitHubStore {
    pub fn new(http: Client, token: String, owner: String, repo: String, branch: String) -> Self {
        Self {
            http,
            token,
            owner,
            repo,
            branch,
        }
    }

    fn contents_url(&self, path: &str) -> String {
        format!(
            "https://api.github.com/repos/{}/{}/contents/{}",
            self.owner, self.repo, path
        )
    }

    fn request(&self, method: reqwest::Method, url: String) -> reqwest::RequestBuilder {
        self.http
            .request(method, url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "telenotes-bot")
    }

    /// Commits `bytes` to `path`. Paths are built from Telegram's
    /// `file_unique_id`, so in practice this always creates; we still look up
    /// an existing sha so that re-sending the same file updates cleanly
    /// instead of failing with a 422.
    pub async fn put(&self, path: &str, bytes: &[u8], message: &str) -> Result<()> {
        if bytes.len() > MAX_FILE_BYTES {
            bail!(
                "file is {}MB, over the 45MB ceiling for GitHub's Contents API",
                bytes.len() / 1024 / 1024
            );
        }

        let mut body = json!({
            "message": message,
            "content": base64::engine::general_purpose::STANDARD.encode(bytes),
            "branch": self.branch,
        });
        if let Some(sha) = self.sha_of(path).await? {
            body["sha"] = json!(sha);
        }

        let resp = self
            .request(reqwest::Method::PUT, self.contents_url(path))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("GitHub upload failed ({status}): {text}");
        }
        Ok(())
    }

    pub async fn get(&self, path: &str) -> Result<Vec<u8>> {
        let resp = self
            .request(reqwest::Method::GET, self.contents_url(path))
            .header("Accept", "application/vnd.github.raw+json")
            .query(&[("ref", self.branch.as_str())])
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("GitHub download failed ({status}): {text}");
        }
        Ok(resp.bytes().await?.to_vec())
    }

    /// Removes `path` from the branch tip. The blob remains in git history --
    /// see the module comment.
    pub async fn delete(&self, path: &str, message: &str) -> Result<()> {
        let Some(sha) = self.sha_of(path).await? else {
            return Ok(()); // already gone
        };

        let resp = self
            .request(reqwest::Method::DELETE, self.contents_url(path))
            .json(&json!({ "message": message, "sha": sha, "branch": self.branch }))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("GitHub delete failed ({status}): {text}");
        }
        Ok(())
    }

    /// Blob sha for `path`, or `None` if it doesn't exist. The Contents API
    /// needs this to update or delete.
    async fn sha_of(&self, path: &str) -> Result<Option<String>> {
        let resp = self
            .request(reqwest::Method::GET, self.contents_url(path))
            .query(&[("ref", self.branch.as_str())])
            .send()
            .await?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("GitHub metadata lookup failed ({status}): {text}");
        }

        let value: serde_json::Value = resp.json().await?;
        Ok(value
            .get("sha")
            .and_then(|s| s.as_str())
            .map(str::to_string))
    }
}
