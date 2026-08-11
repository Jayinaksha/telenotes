use anyhow::Result;
use chrono::{Duration, Utc};
use deadpool_postgres::{Config, Pool, Runtime};
use rand::Rng;
use uuid::Uuid;

use crate::models::{Item, Stats};
use crate::storage::Backend;

/// Columns selected for `row_to_item`. Listed explicitly rather than `SELECT
/// *` so that adding a column to the table can't silently change what the
/// mapper sees.
const ITEM_COLUMNS: &str = "id, user_id, kind, content_text, file_id, file_unique_id, \
                            backend, storage_path, file_name, mime_type, file_size, \
                            tags, created_at";

pub fn create_pool(database_url: &str) -> Result<Pool> {
    let mut cfg = Config::new();
    cfg.url = Some(database_url.to_string());

    let tls_connector = native_tls::TlsConnector::builder().build()?;
    let tls = postgres_native_tls::MakeTlsConnector::new(tls_connector);

    Ok(cfg.create_pool(Some(Runtime::Tokio1), tls)?)
}

/// Cheap query behind `/health`. Keeps the Supabase project from auto-pausing
/// after 7 days of no database activity.
pub async fn health_check(pool: &Pool) -> Result<()> {
    let client = pool.get().await?;
    client.query_one("SELECT 1", &[]).await?;
    Ok(())
}

pub async fn ensure_user(pool: &Pool, chat_id: i64, username: Option<&str>) -> Result<Uuid> {
    let client = pool.get().await?;
    let row = client
        .query_one(
            "INSERT INTO users (telegram_chat_id, username)
             VALUES ($1, $2)
             ON CONFLICT (telegram_chat_id)
             DO UPDATE SET username = EXCLUDED.username
             RETURNING id",
            &[&chat_id, &username],
        )
        .await?;
    Ok(row.get("id"))
}

/// Takes an explicit `id` because the caller derives the note's markdown path
/// from it, and wants that path stable before the row exists.
pub async fn insert_note(
    pool: &Pool,
    id: Uuid,
    user_id: Uuid,
    content_text: &str,
    tags: &[String],
    storage_path: &str,
) -> Result<()> {
    let client = pool.get().await?;
    client
        .execute(
            "INSERT INTO items (id, user_id, kind, content_text, tags, backend, storage_path)
             VALUES ($1, $2, 'note', $3, $4, 'github', $5)",
            &[&id, &user_id, &content_text, &tags, &storage_path],
        )
        .await?;
    Ok(())
}

pub struct NewFile<'a> {
    pub kind: &'a str,
    pub content_text: Option<&'a str>,
    pub file_id: &'a str,
    pub file_unique_id: &'a str,
    pub file_name: Option<&'a str>,
    pub mime_type: Option<&'a str>,
    pub file_size: Option<i64>,
    pub backend: Backend,
    pub storage_path: &'a str,
    pub tags: &'a [String],
}

pub async fn insert_file(pool: &Pool, user_id: Uuid, f: NewFile<'_>) -> Result<Uuid> {
    let client = pool.get().await?;
    let row = client
        .query_one(
            "INSERT INTO items
                (user_id, kind, content_text, file_id, file_unique_id,
                 file_name, mime_type, file_size, backend, storage_path, tags)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
             RETURNING id",
            &[
                &user_id,
                &f.kind,
                &f.content_text,
                &f.file_id,
                &f.file_unique_id,
                &f.file_name,
                &f.mime_type,
                &f.file_size,
                &f.backend.as_str(),
                &f.storage_path,
                &f.tags,
            ],
        )
        .await?;
    Ok(row.get("id"))
}

fn row_to_item(row: &tokio_postgres::Row) -> Item {
    Item {
        id: row.get("id"),
        user_id: row.get("user_id"),
        kind: row.get("kind"),
        content_text: row.get("content_text"),
        file_id: row.get("file_id"),
        file_unique_id: row.get("file_unique_id"),
        backend: row.get("backend"),
        storage_path: row.get("storage_path"),
        file_name: row.get("file_name"),
        mime_type: row.get("mime_type"),
        file_size: row.get("file_size"),
        tags: row.get("tags"),
        created_at: row.get("created_at"),
    }
}

pub async fn list_items(
    pool: &Pool,
    user_id: Uuid,
    kind_filter: Option<&str>,
    tag_filter: Option<&str>,
    limit: i64,
) -> Result<Vec<Item>> {
    let client = pool.get().await?;
    let sql = format!(
        "SELECT {ITEM_COLUMNS} FROM items
         WHERE user_id = $1
           AND ($2::text IS NULL OR kind = $2)
           AND ($3::text IS NULL OR $3 = ANY(tags))
         ORDER BY created_at DESC
         LIMIT $4"
    );
    let rows = client
        .query(&sql, &[&user_id, &kind_filter, &tag_filter, &limit])
        .await?;
    Ok(rows.iter().map(row_to_item).collect())
}

/// Everything that isn't a note, optionally filtered by tag. Done in SQL
/// rather than by fetching and filtering in Rust, so `LIMIT` means what it
/// looks like it means.
pub async fn list_files(
    pool: &Pool,
    user_id: Uuid,
    tag_filter: Option<&str>,
    limit: i64,
) -> Result<Vec<Item>> {
    let client = pool.get().await?;
    let sql = format!(
        "SELECT {ITEM_COLUMNS} FROM items
         WHERE user_id = $1
           AND kind <> 'note'
           AND ($2::text IS NULL OR $2 = ANY(tags))
         ORDER BY created_at DESC
         LIMIT $3"
    );
    let rows = client.query(&sql, &[&user_id, &tag_filter, &limit]).await?;
    Ok(rows.iter().map(row_to_item).collect())
}

pub async fn search_items(
    pool: &Pool,
    user_id: Uuid,
    query: &str,
    limit: i64,
) -> Result<Vec<Item>> {
    let client = pool.get().await?;
    let sql = format!(
        "SELECT {ITEM_COLUMNS} FROM items
         WHERE user_id = $1
           AND search_vector @@ plainto_tsquery('english', $2)
         ORDER BY ts_rank(search_vector, plainto_tsquery('english', $2)) DESC,
                  created_at DESC
         LIMIT $3"
    );
    let rows = client.query(&sql, &[&user_id, &query, &limit]).await?;
    Ok(rows.iter().map(row_to_item).collect())
}

/// Looks an item up by the 8-character prefix shown in listings.
///
/// The prefix is validated as hex before it reaches SQL: `LIKE` treats `%`
/// and `_` as wildcards, so an unvalidated argument would let `/get %` match
/// an arbitrary row, and `/get` with no argument match everything.
pub async fn get_item_by_short_id(
    pool: &Pool,
    user_id: Uuid,
    short_id: &str,
) -> Result<Option<Item>> {
    let trimmed = short_id.trim();
    if trimmed.is_empty()
        || trimmed.len() > 36
        || !trimmed.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
    {
        return Ok(None);
    }

    let client = pool.get().await?;
    let sql = format!(
        "SELECT {ITEM_COLUMNS} FROM items
         WHERE user_id = $1 AND id::text LIKE $2 || '%'
         LIMIT 1"
    );
    let row = client.query_opt(&sql, &[&user_id, &trimmed]).await?;
    Ok(row.as_ref().map(row_to_item))
}

pub async fn delete_item(pool: &Pool, user_id: Uuid, id: Uuid) -> Result<bool> {
    let client = pool.get().await?;
    let n = client
        .execute(
            "DELETE FROM items WHERE user_id = $1 AND id = $2",
            &[&user_id, &id],
        )
        .await?;
    Ok(n > 0)
}

pub async fn list_tags(pool: &Pool, user_id: Uuid) -> Result<Vec<(String, i64)>> {
    let client = pool.get().await?;
    let rows = client
        .query(
            "SELECT tag, count(*) AS n
             FROM items, unnest(tags) AS tag
             WHERE user_id = $1
             GROUP BY tag
             ORDER BY n DESC, tag ASC",
            &[&user_id],
        )
        .await?;
    Ok(rows.iter().map(|r| (r.get("tag"), r.get("n"))).collect())
}

pub async fn stats(pool: &Pool, user_id: Uuid) -> Result<Stats> {
    let client = pool.get().await?;
    let row = client
        .query_one(
            "SELECT
               count(*) FILTER (WHERE kind = 'note')  AS notes,
               count(*) FILTER (WHERE kind <> 'note') AS files,
               coalesce(sum(file_size), 0)            AS bytes_stored,
               count(DISTINCT t)                      AS tags
             FROM items LEFT JOIN LATERAL unnest(tags) AS t ON true
             WHERE user_id = $1",
            &[&user_id],
        )
        .await?;
    Ok(Stats {
        notes: row.get("notes"),
        files: row.get("files"),
        bytes_stored: row.get("bytes_stored"),
        tags: row.get("tags"),
    })
}

/// Issues a short code the web UI can redeem to bind this Telegram account to
/// a Supabase Auth login. Codes are single-use and expire quickly.
pub async fn create_link_code(pool: &Pool, user_id: Uuid) -> Result<(String, i64)> {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789"; // no I/O/0/1
    const TTL_MINUTES: i64 = 10;

    let code: String = {
        let mut rng = rand::thread_rng();
        (0..8)
            .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
            .collect()
    };

    let expires_at = Utc::now() + Duration::minutes(TTL_MINUTES);
    let client = pool.get().await?;

    // Only one live code per user; issuing a new one invalidates the old.
    client
        .execute(
            "DELETE FROM link_codes WHERE user_id = $1 AND used_at IS NULL",
            &[&user_id],
        )
        .await?;
    client
        .execute(
            "INSERT INTO link_codes (code, user_id, expires_at) VALUES ($1, $2, $3)",
            &[&code, &user_id, &expires_at],
        )
        .await?;

    Ok((code, TTL_MINUTES))
}

/// Housekeeping for expired codes and tokens, called opportunistically from
/// /health (which the keep-alive cron hits regularly anyway).
pub async fn purge_expired(pool: &Pool) -> Result<()> {
    let client = pool.get().await?;
    client
        .execute("DELETE FROM link_codes WHERE expires_at < now()", &[])
        .await?;
    client
        .execute("DELETE FROM download_tokens WHERE expires_at < now()", &[])
        .await?;
    Ok(())
}

/// Redeems a download token minted by `issue_download_token`. Returns the item
/// only while the token is unexpired; authorization already happened in
/// Postgres, so there is nothing further to check here.
pub async fn item_for_download_token(pool: &Pool, token: &str) -> Result<Option<Item>> {
    if token.len() != 48 || !token.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(None);
    }
    let client = pool.get().await?;
    let sql = format!(
        "SELECT {ITEM_COLUMNS} FROM items i
         WHERE i.id = (
           SELECT dt.item_id FROM download_tokens dt
           WHERE dt.token = $1 AND dt.expires_at > now()
         )"
    );
    let row = client.query_opt(&sql, &[&token]).await?;
    Ok(row.as_ref().map(row_to_item))
}
