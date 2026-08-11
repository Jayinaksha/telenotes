use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub id: Uuid,
    pub user_id: Uuid,
    pub kind: String,
    pub content_text: Option<String>,
    pub file_id: Option<String>,
    pub file_unique_id: Option<String>,
    pub backend: Option<String>,
    pub storage_path: Option<String>,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    pub file_size: Option<i64>,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
}

impl Item {
    pub fn short_id(&self) -> String {
        self.id.to_string()[..8].to_string()
    }

    pub fn backend(&self) -> Backend {
        Backend::parse(self.backend.as_deref())
    }

    /// One-line summary used in /notes, /files and /search listings, and by
    /// the TUI's list view.
    pub fn summary(&self) -> String {
        let short_id = self.short_id();
        match self.kind.as_str() {
            "note" => {
                let text = self.content_text.clone().unwrap_or_default();
                // Collapse newlines so a multi-line note stays on one row.
                let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
                let preview: String = flat.chars().take(60).collect();
                let ellipsis = if flat.chars().count() > 60 { "…" } else { "" };
                format!("{short_id}  {preview}{ellipsis}")
            }
            _ => {
                let name = self
                    .file_name
                    .clone()
                    .unwrap_or_else(|| format!("{} file", self.kind));
                format!("{short_id}  [{}] {}", self.kind, name)
            }
        }
    }
}

/// Counts shown by /stats and the dashboard/TUI stats panel.
pub struct Stats {
    pub notes: i64,
    pub files: i64,
    pub bytes_stored: i64,
    pub tags: i64,
}
