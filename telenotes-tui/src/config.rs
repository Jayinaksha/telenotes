//! Three env vars, same information `web/config.js` holds for the
//! dashboard: where Supabase is, its public anon key, and the bot's URL.

use anyhow::{anyhow, Result};

pub struct Config {
    pub supabase_url: String,
    pub supabase_anon_key: String,
    pub bot_url: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            supabase_url: req("TELENOTES_SUPABASE_URL")?,
            supabase_anon_key: req("TELENOTES_SUPABASE_ANON_KEY")?,
            bot_url: req("TELENOTES_BOT_URL")?,
        })
    }
}

fn req(key: &str) -> Result<String> {
    std::env::var(key).map_err(|_| {
        anyhow!(
            "environment variable {key} must be set (same values as web/config.js -- \
             see SETUP.md, \"The TUI client\")"
        )
    })
}
