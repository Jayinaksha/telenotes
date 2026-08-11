//! Storage routing.
//!
//! Two backends, picked by what's being stored rather than by configuration
//! guesswork:
//!
//! * **Notes** always go to GitHub, written as markdown with YAML
//!   frontmatter. The vault repo stays human-readable and diffable, and you
//!   keep a full edit history for free.
//! * **Files** go to R2 when it's configured, because git is a poor place for
//!   binaries that you may later want to delete. Without R2 they fall back to
//!   GitHub, so the bot is fully functional on GitHub alone.

pub mod github;
pub mod r2;

use std::time::Duration;

use anyhow::{bail, Result};

pub use github::GitHubStore;
pub use r2::R2Store;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    GitHub,
    R2,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::GitHub => "github",
            Backend::R2 => "r2",
        }
    }

    /// Parses the `backend` column. Rows written before R2 support existed
    /// have no value, and those were all GitHub.
    pub fn parse(s: Option<&str>) -> Self {
        match s {
            Some("r2") => Backend::R2,
            _ => Backend::GitHub,
        }
    }
}

#[derive(Clone)]
pub struct Vault {
    github: GitHubStore,
    r2: Option<R2Store>,
}

impl Vault {
    pub fn new(github: GitHubStore, r2: Option<R2Store>) -> Self {
        Self { github, r2 }
    }

    pub fn has_r2(&self) -> bool {
        self.r2.is_some()
    }

    /// Where a binary attachment should be written.
    pub fn backend_for_files(&self) -> Backend {
        if self.r2.is_some() {
            Backend::R2
        } else {
            Backend::GitHub
        }
    }

    /// Writes a note's markdown into the GitHub vault.
    pub async fn put_note(&self, path: &str, markdown: &str, message: &str) -> Result<()> {
        self.github.put(path, markdown.as_bytes(), message).await
    }

    /// Writes an attachment to whichever backend is active, returning the one
    /// used so it can be recorded alongside the row.
    pub async fn put_file(
        &self,
        path: &str,
        bytes: Vec<u8>,
        content_type: &str,
        message: &str,
    ) -> Result<Backend> {
        match &self.r2 {
            Some(r2) => {
                r2.put(path, bytes, content_type).await?;
                Ok(Backend::R2)
            }
            None => {
                self.github.put(path, &bytes, message).await?;
                Ok(Backend::GitHub)
            }
        }
    }

    pub async fn get(&self, backend: Backend, path: &str) -> Result<Vec<u8>> {
        match backend {
            Backend::GitHub => self.github.get(path).await,
            Backend::R2 => match &self.r2 {
                Some(r2) => r2.get(path).await,
                None => bail!(
                    "item is stored in R2 but R2 is not configured on this deploy; \
                     set the R2_* environment variables to reach it"
                ),
            },
        }
    }

    pub async fn delete(&self, backend: Backend, path: &str, message: &str) -> Result<()> {
        match backend {
            Backend::GitHub => self.github.delete(path, message).await,
            Backend::R2 => match &self.r2 {
                Some(r2) => r2.delete(path).await,
                // Nothing we can do without credentials, and failing the whole
                // delete would strand the metadata row too.
                None => Ok(()),
            },
        }
    }

    /// A browser-usable download link, when the backend supports one.
    pub fn presigned_get(&self, backend: Backend, path: &str, ttl: Duration) -> Option<String> {
        match (backend, &self.r2) {
            (Backend::R2, Some(r2)) => Some(r2.presigned_get(path, ttl)),
            _ => None,
        }
    }
}
