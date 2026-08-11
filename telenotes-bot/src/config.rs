//! All environment parsing lives here, so a misconfigured deploy fails at
//! startup with a readable message instead of panicking somewhere deep in a
//! request handler.

use anyhow::{anyhow, bail, Context, Result};

pub struct Config {
    pub bot_token: String,
    pub webhook_url: String,
    pub webhook_secret: String,
    pub port: u16,
    pub database_url: String,
    /// Telegram chat ids permitted to use the bot. Empty means "allow
    /// anyone", which is almost never what you want for a personal vault --
    /// `main` logs a loud warning in that case.
    pub allowed_chat_ids: Vec<i64>,
    pub github: GitHubConfig,
    pub r2: Option<R2Config>,
    pub notion: Option<NotionConfig>,
    /// The dashboard's origin (e.g. `https://you.github.io`), the only
    /// origin the `/api/*` write endpoints answer CORS preflights for.
    pub dashboard_origin: String,
}

pub struct GitHubConfig {
    pub token: String,
    pub owner: String,
    pub repo: String,
    pub branch: String,
}

pub struct R2Config {
    pub account_id: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub bucket: String,
}

pub struct NotionConfig {
    pub token: String,
    pub database_id: String,
}

// Trimmed: a copy-pasted value picking up a trailing newline or space is an
// easy, invisible mistake (e.g. in a dashboard's env var text box), and
// something like a connection string silently growing a `\n` produces a
// deeply confusing downstream error rather than an obvious one here.

fn req(key: &str) -> Result<String> {
    let value =
        std::env::var(key).map_err(|_| anyhow!("environment variable {key} must be set"))?;
    Ok(value.trim().to_string())
}

fn opt(key: &str) -> Option<String> {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
        _ => None,
    }
}

/// Telegram's `setWebhook` rejects anything outside this set with an opaque
/// API error rather than a helpful message -- catch it here instead, since
/// Render's auto-generated env var values aren't guaranteed to satisfy it
/// (standard base64 output, for instance, can include `+`, `/`, `=`).
fn validate_webhook_secret(secret: &str) -> Result<()> {
    let len = secret.chars().count();
    if len == 0 || len > 256 {
        bail!("WEBHOOK_SECRET must be 1-256 characters, got {len}");
    }
    if !secret
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        bail!(
            "WEBHOOK_SECRET contains characters Telegram's setWebhook doesn't allow \
             (only A-Z, a-z, 0-9, '_', '-'). If this came from a host's auto-generated \
             value, it may include '+', '/', or '=' from base64 -- replace it with, \
             e.g., the output of `openssl rand -hex 32` instead."
        );
    }
    Ok(())
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let allowed_chat_ids = match opt("ALLOWED_CHAT_IDS") {
            None => Vec::new(),
            Some(raw) => raw
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| {
                    s.parse::<i64>().with_context(|| {
                        format!("ALLOWED_CHAT_IDS contains a non-numeric id: {s:?}")
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        };

        // R2 is optional. It's all-or-nothing: a half-filled set of R2
        // variables is a deploy mistake worth failing on rather than
        // silently falling back to GitHub for files.
        let r2_vars = [
            opt("R2_ACCOUNT_ID"),
            opt("R2_ACCESS_KEY_ID"),
            opt("R2_SECRET_ACCESS_KEY"),
            opt("R2_BUCKET"),
        ];
        let r2 = match r2_vars.iter().filter(|v| v.is_some()).count() {
            0 => None,
            4 => Some(R2Config {
                account_id: r2_vars[0].clone().unwrap(),
                access_key_id: r2_vars[1].clone().unwrap(),
                secret_access_key: r2_vars[2].clone().unwrap(),
                bucket: r2_vars[3].clone().unwrap(),
            }),
            n => {
                return Err(anyhow!(
                    "R2 is partially configured ({n}/4 variables set). Set all of \
                     R2_ACCOUNT_ID, R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY, R2_BUCKET, \
                     or none of them to store everything in GitHub."
                ))
            }
        };

        let notion = match (opt("NOTION_TOKEN"), opt("NOTION_DATABASE_ID")) {
            (Some(token), Some(database_id)) => Some(NotionConfig { token, database_id }),
            (None, None) => None,
            _ => {
                return Err(anyhow!(
                    "Notion is partially configured. Set both NOTION_TOKEN and \
                     NOTION_DATABASE_ID, or neither."
                ))
            }
        };

        let webhook_secret = req("WEBHOOK_SECRET")?;
        validate_webhook_secret(&webhook_secret)?;

        Ok(Config {
            bot_token: req("BOT_TOKEN")?,
            webhook_url: req("WEBHOOK_URL")?,
            webhook_secret,
            port: opt("PORT")
                .unwrap_or_else(|| "8080".to_string())
                .parse()
                .context("PORT must be a number")?,
            database_url: req("DATABASE_URL")?,
            allowed_chat_ids,
            github: GitHubConfig {
                token: req("GITHUB_TOKEN")?,
                owner: req("GITHUB_OWNER")?,
                repo: req("GITHUB_REPO")?,
                branch: opt("GITHUB_BRANCH").unwrap_or_else(|| "main".to_string()),
            },
            r2,
            notion,
            dashboard_origin: req("DASHBOARD_ORIGIN")?,
        })
    }

    pub fn is_allowed(&self, chat_id: i64) -> bool {
        self.allowed_chat_ids.is_empty() || self.allowed_chat_ids.contains(&chat_id)
    }
}
