//! Cloudflare R2 backend, spoken to over the S3-compatible API.
//!
//! R2's free tier is 10GB of storage, 1M writes and 10M reads per month, with
//! no egress charges at all -- which is what makes it a better home than git
//! for photos, voice notes and video. Deletes actually reclaim space here.
//!
//! We only ever presign URLs and drive them with `reqwest`, so there's no
//! heavyweight AWS SDK in the dependency tree.

use std::time::Duration;

use anyhow::{bail, Result};
use reqwest::Client;
use rusty_s3::actions::{DeleteObject, GetObject, PutObject};
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};
use url::Url;

/// How long a presigned URL stays valid. Short for the ones we use
/// server-side; [`R2Store::presigned_get`] takes its own lifetime because
/// those get handed to a browser.
const SIGN_TTL: Duration = Duration::from_secs(300);

#[derive(Clone)]
pub struct R2Store {
    http: Client,
    bucket: Bucket,
    credentials: Credentials,
}

impl R2Store {
    pub fn new(
        http: Client,
        account_id: &str,
        access_key_id: &str,
        secret_access_key: &str,
        bucket: &str,
    ) -> Result<Self> {
        let endpoint: Url = format!("https://{account_id}.r2.cloudflarestorage.com").parse()?;
        // R2 ignores the region but SigV4 requires one; "auto" is what
        // Cloudflare documents for S3 clients.
        let bucket = Bucket::new(endpoint, UrlStyle::Path, bucket.to_string(), "auto")?;
        let credentials = Credentials::new(access_key_id, secret_access_key);
        Ok(Self {
            http,
            bucket,
            credentials,
        })
    }

    pub async fn put(&self, path: &str, bytes: Vec<u8>, content_type: &str) -> Result<()> {
        let action = PutObject::new(&self.bucket, Some(&self.credentials), path);
        let url = action.sign(SIGN_TTL);

        let resp = self
            .http
            .put(url)
            .header("Content-Type", content_type)
            .body(bytes)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("R2 upload failed ({status}): {text}");
        }
        Ok(())
    }

    pub async fn get(&self, path: &str) -> Result<Vec<u8>> {
        let action = GetObject::new(&self.bucket, Some(&self.credentials), path);
        let resp = self.http.get(action.sign(SIGN_TTL)).send().await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("R2 download failed ({status}): {text}");
        }
        Ok(resp.bytes().await?.to_vec())
    }

    pub async fn delete(&self, path: &str) -> Result<()> {
        let action = DeleteObject::new(&self.bucket, Some(&self.credentials), path);
        let resp = self.http.delete(action.sign(SIGN_TTL)).send().await?;

        // S3 delete is idempotent: a missing key still returns 204.
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("R2 delete failed ({status}): {text}");
        }
        Ok(())
    }

    /// A time-limited download URL safe to hand to a browser. This is how the
    /// web UI fetches attachments without ever seeing the R2 credentials.
    pub fn presigned_get(&self, path: &str, ttl: Duration) -> String {
        GetObject::new(&self.bucket, Some(&self.credentials), path)
            .sign(ttl)
            .to_string()
    }
}
