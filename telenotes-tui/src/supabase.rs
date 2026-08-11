//! Talks to Supabase directly: Auth REST for login, PostgREST for reads and
//! for minting write-capability tokens. Reads hit exactly the same
//! tables/RPCs the dashboard uses, scoped by the same RLS policies --
//! PostgREST doesn't distinguish `supabase-js` from `reqwest`.

use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use telenotes_core::Item;
use uuid::Uuid;

use crate::config::Config;
use crate::session::Session;

const ITEM_COLUMNS: &str = "id,user_id,kind,content_text,file_id,file_unique_id,backend,\
                             storage_path,file_name,mime_type,file_size,tags,created_at";

pub struct SupabaseClient {
    http: reqwest::Client,
    config: Config,
}

#[derive(Debug, Deserialize)]
pub struct TagCount {
    pub tag: String,
    pub n: i64,
}

#[derive(Debug, Deserialize)]
pub struct DayCount {
    pub day: String,
    pub n: i64,
}

#[derive(Debug, Deserialize)]
pub struct StorageRow {
    pub kind: String,
    pub backend: String,
    pub n: i64,
    pub bytes: i64,
}

#[derive(Deserialize)]
struct AuthResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
}

impl SupabaseClient {
    pub fn new(config: Config) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent("telenotes-tui")
                .build()?,
            config,
        })
    }

    pub fn bot_url(&self) -> &str {
        &self.config.bot_url
    }

    /// Sends the login email. Requires the Supabase project's Magic Link
    /// email template to include `{{ .Token }}` -- see SETUP.md, "The TUI
    /// client". The dashboard is unaffected; it uses the link, not the code.
    pub async fn request_otp(&self, email: &str) -> Result<()> {
        let resp = self
            .http
            .post(format!("{}/auth/v1/otp", self.config.supabase_url))
            .header("apikey", &self.config.supabase_anon_key)
            .json(&json!({ "email": email, "create_user": false }))
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!(
                "could not send login code: {}",
                resp.text().await.unwrap_or_default()
            );
        }
        Ok(())
    }

    pub async fn verify_otp(&self, email: &str, code: &str) -> Result<Session> {
        let resp = self
            .http
            .post(format!("{}/auth/v1/verify", self.config.supabase_url))
            .header("apikey", &self.config.supabase_anon_key)
            .json(&json!({ "email": email, "token": code, "type": "email" }))
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!(
                "invalid or expired code: {}",
                resp.text().await.unwrap_or_default()
            );
        }
        let auth: AuthResponse = resp.json().await?;
        Ok(Session {
            email: email.to_string(),
            access_token: auth.access_token,
            refresh_token: auth.refresh_token,
            expires_at: Utc::now().timestamp() + auth.expires_in,
        })
    }

    pub async fn refresh(&self, session: &Session) -> Result<Session> {
        let resp = self
            .http
            .post(format!(
                "{}/auth/v1/token?grant_type=refresh_token",
                self.config.supabase_url
            ))
            .header("apikey", &self.config.supabase_anon_key)
            .json(&json!({ "refresh_token": session.refresh_token }))
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!(
                "session refresh failed: {}",
                resp.text().await.unwrap_or_default()
            );
        }
        let auth: AuthResponse = resp.json().await?;
        Ok(Session {
            email: session.email.clone(),
            access_token: auth.access_token,
            refresh_token: auth.refresh_token,
            expires_at: Utc::now().timestamp() + auth.expires_in,
        })
    }

    fn authed(&self, req: reqwest::RequestBuilder, session: &Session) -> reqwest::RequestBuilder {
        req.header("apikey", &self.config.supabase_anon_key)
            .header("Authorization", format!("Bearer {}", session.access_token))
    }

    pub async fn list_items(&self, session: &Session) -> Result<Vec<Item>> {
        let url = format!(
            "{}/rest/v1/items?select={ITEM_COLUMNS}&order=created_at.desc&limit=200",
            self.config.supabase_url
        );
        parse(self.authed(self.http.get(url), session).send().await?).await
    }

    pub async fn search_items(&self, session: &Session, query: &str) -> Result<Vec<Item>> {
        let url = format!("{}/rest/v1/rpc/search_items", self.config.supabase_url);
        let resp = self
            .authed(self.http.post(url), session)
            .json(&json!({ "q": query, "lim": 200 }))
            .send()
            .await?;
        parse(resp).await
    }

    pub async fn tag_counts(&self, session: &Session) -> Result<Vec<TagCount>> {
        let url = format!("{}/rest/v1/rpc/tag_counts", self.config.supabase_url);
        let resp = self
            .authed(self.http.post(url), session)
            .json(&json!({}))
            .send()
            .await?;
        parse(resp).await
    }

    pub async fn items_per_day(&self, session: &Session, days: i32) -> Result<Vec<DayCount>> {
        let url = format!("{}/rest/v1/rpc/items_per_day", self.config.supabase_url);
        let resp = self
            .authed(self.http.post(url), session)
            .json(&json!({ "days": days }))
            .send()
            .await?;
        parse(resp).await
    }

    pub async fn storage_breakdown(&self, session: &Session) -> Result<Vec<StorageRow>> {
        let url = format!("{}/rest/v1/rpc/storage_breakdown", self.config.supabase_url);
        let resp = self
            .authed(self.http.post(url), session)
            .json(&json!({}))
            .send()
            .await?;
        parse(resp).await
    }

    /// Mints a bot-write capability token -- the same `issue_write_token` RPC
    /// the dashboard calls via `supabase.rpc(...)`. There is no TUI-specific
    /// token path: PostgREST doesn't care whether the caller is a browser or
    /// `reqwest`.
    pub async fn issue_write_token(
        &self,
        session: &Session,
        purpose: &str,
        item_id: Option<Uuid>,
    ) -> Result<String> {
        let url = format!("{}/rest/v1/rpc/issue_write_token", self.config.supabase_url);
        let resp = self
            .authed(self.http.post(url), session)
            .json(&json!({ "purpose": purpose, "item_id": item_id }))
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!(
                "could not get permission for this action: {}",
                resp.text().await.unwrap_or_default()
            );
        }
        resp.json()
            .await
            .context("unexpected response minting a write token")
    }

    /// Same shape as `issue_write_token`, for the pre-existing download flow
    /// (`main.rs`'s `/d/{token}` on the bot).
    pub async fn issue_download_token(&self, session: &Session, item_id: Uuid) -> Result<String> {
        let url = format!(
            "{}/rest/v1/rpc/issue_download_token",
            self.config.supabase_url
        );
        let resp = self
            .authed(self.http.post(url), session)
            .json(&json!({ "item_id": item_id }))
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!(
                "could not prepare download: {}",
                resp.text().await.unwrap_or_default()
            );
        }
        resp.json()
            .await
            .context("unexpected response minting a download token")
    }
}

async fn parse<T: serde::de::DeserializeOwned>(resp: reqwest::Response) -> Result<T> {
    if !resp.status().is_success() {
        bail!(
            "Supabase request failed: {}",
            resp.text().await.unwrap_or_default()
        );
    }
    resp.json().await.context("unexpected response shape")
}
