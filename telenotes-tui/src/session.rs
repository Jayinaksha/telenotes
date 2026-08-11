//! A logged-in Supabase Auth session, persisted to
//! `~/.config/telenotes/session.json` so the OTP login isn't repeated every
//! launch.

use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub email: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds.
    pub expires_at: i64,
}

impl Session {
    fn path() -> Result<PathBuf> {
        let dir = dirs::config_dir()
            .context("could not determine a config directory")?
            .join("telenotes");
        Ok(dir.join("session.json"))
    }

    pub async fn load() -> Option<Session> {
        let path = Self::path().ok()?;
        let bytes = tokio::fs::read(&path).await.ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// `chmod 600` after writing -- this file holds a bearer credential.
    pub async fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        let bytes = serde_json::to_vec_pretty(self)?;
        tokio::fs::write(&path, &bytes).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).await?;
        }
        Ok(())
    }

    /// Refresh proactively once fewer than 60 seconds remain, rather than
    /// waiting for a request to come back `401`.
    pub fn is_near_expiry(&self) -> bool {
        self.expires_at - Utc::now().timestamp() < 60
    }
}
