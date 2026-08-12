use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
use deadpool_postgres::{Config, Pool, Runtime};
use rand::Rng;
use telenotes_core::{Item, Stats};
use uuid::Uuid;

use crate::storage::Backend;

/// Columns selected for `row_to_item`. Listed explicitly rather than `SELECT
/// *` so that adding a column to the table can't silently change what the
/// mapper sees.
const ITEM_COLUMNS: &str = "id, user_id, kind, content_text, file_id, file_unique_id, \
                            backend, storage_path, file_name, mime_type, file_size, \
                            tags, created_at";

/// Supabase's connection pooler presents a certificate chain rooted at
/// Supabase's own CA, not one that chains to a public root -- neither the
/// OS trust store (native-tls) nor Mozilla's bundled roots (webpki-roots)
/// will ever trust it. Pin it explicitly so the connection is still fully
/// verified rather than falling back to skipping verification.
const SUPABASE_ROOT_CA_PEM: &[u8] = include_bytes!("../certs/supabase-root-2021-ca.pem");

pub fn create_pool(database_url: &str) -> Result<Pool> {
    let mut cfg = Config::new();
    cfg.url = Some(database_url.to_string());

    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let supabase_ca = rustls_pemfile::certs(&mut &SUPABASE_ROOT_CA_PEM[..])
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("bundled Supabase root CA is not valid PEM")?;
    let (added, _) = root_store.add_parsable_certificates(supabase_ca);
    if added == 0 {
        bail!("bundled Supabase root CA did not parse as a certificate");
    }

    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config);

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
) -> Result<DateTime<Utc>> {
    let client = pool.get().await?;
    let row = client
        .query_one(
            "INSERT INTO items (id, user_id, kind, content_text, tags, backend, storage_path)
             VALUES ($1, $2, 'note', $3, $4, 'github', $5)
             RETURNING created_at",
            &[&id, &user_id, &content_text, &tags, &storage_path],
        )
        .await?;
    Ok(row.get("created_at"))
}

pub struct NewFile<'a> {
    pub kind: &'a str,
    pub content_text: Option<&'a str>,
    /// Telegram provenance. `None` for items created via the web/TUI write
    /// API, which have no Telegram file to reference.
    pub file_id: Option<&'a str>,
    pub file_unique_id: Option<&'a str>,
    pub file_name: Option<&'a str>,
    pub mime_type: Option<&'a str>,
    pub file_size: Option<i64>,
    pub backend: Backend,
    pub storage_path: &'a str,
    pub tags: &'a [String],
}

pub async fn insert_file(
    pool: &Pool,
    user_id: Uuid,
    f: NewFile<'_>,
) -> Result<(Uuid, DateTime<Utc>)> {
    let client = pool.get().await?;
    let row = client
        .query_one(
            "INSERT INTO items
                (user_id, kind, content_text, file_id, file_unique_id,
                 file_name, mime_type, file_size, backend, storage_path, tags)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
             RETURNING id, created_at",
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
    Ok((row.get("id"), row.get("created_at")))
}

/// Unscoped by `user_id` -- callers only reach this after already proving
/// ownership via a claimed write token, mirroring how `item_for_download_token`
/// needs no further check once the token itself is valid.
pub async fn get_item(pool: &Pool, id: Uuid) -> Result<Option<Item>> {
    let client = pool.get().await?;
    let sql = format!("SELECT {ITEM_COLUMNS} FROM items WHERE id = $1");
    let row = client.query_opt(&sql, &[&id]).await?;
    Ok(row.as_ref().map(row_to_item))
}

/// `NULL` in any of the optional fields leaves that column untouched, via
/// `COALESCE`. `items_set_updated_at` stamps `updated_at` automatically.
pub async fn update_item(
    pool: &Pool,
    id: Uuid,
    user_id: Uuid,
    content_text: Option<&str>,
    tags: Option<&[String]>,
    file_name: Option<&str>,
    mime_type: Option<&str>,
) -> Result<Option<Item>> {
    let client = pool.get().await?;
    let sql = format!(
        "UPDATE items SET
           content_text = COALESCE($3, content_text),
           tags         = COALESCE($4, tags),
           file_name    = COALESCE($5, file_name),
           mime_type    = COALESCE($6, mime_type)
         WHERE id = $1 AND user_id = $2
         RETURNING {ITEM_COLUMNS}"
    );
    let row = client
        .query_opt(
            &sql,
            &[&id, &user_id, &content_text, &tags, &file_name, &mime_type],
        )
        .await?;
    Ok(row.as_ref().map(row_to_item))
}

pub async fn set_notion_page_id(pool: &Pool, item_id: Uuid, page_id: &str) -> Result<()> {
    let client = pool.get().await?;
    client
        .execute(
            "UPDATE items SET notion_page_id = $2 WHERE id = $1",
            &[&item_id, &page_id],
        )
        .await?;
    Ok(())
}

pub async fn get_notion_page_id(pool: &Pool, item_id: Uuid) -> Result<Option<String>> {
    let client = pool.get().await?;
    let row = client
        .query_opt(
            "SELECT notion_page_id FROM items WHERE id = $1",
            &[&item_id],
        )
        .await?;
    Ok(row.and_then(|r| r.get("notion_page_id")))
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
    client
        .execute("DELETE FROM action_tokens WHERE expires_at < now()", &[])
        .await?;
    Ok(())
}

/// What a claimed write token proves: which user minted it, and (for
/// edit_item/delete_item) which item it's bound to.
pub struct ActionToken {
    pub user_id: Uuid,
    pub item_id: Option<Uuid>,
}

/// Redeems a token minted by `issue_write_token`, for exactly the purpose it
/// was minted for. The `UPDATE ... RETURNING` is the whole single-use
/// enforcement mechanism: it's atomic under Postgres's row lock, so two
/// concurrent requests racing the same token can't both succeed -- the loser
/// gets zero rows back, same as an already-used or expired token.
pub async fn claim_write_token(
    pool: &Pool,
    token: &str,
    expected_purpose: &str,
) -> Result<Option<ActionToken>> {
    if token.len() != 48 || !token.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(None);
    }
    let client = pool.get().await?;
    let row = client
        .query_opt(
            "UPDATE action_tokens
             SET used_at = now()
             WHERE token = $1 AND purpose = $2 AND used_at IS NULL AND expires_at > now()
             RETURNING user_id, item_id",
            &[&token, &expected_purpose],
        )
        .await?;
    Ok(row.as_ref().map(|r| ActionToken {
        user_id: r.get("user_id"),
        item_id: r.get("item_id"),
    }))
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

// ---------------------------------------------------------------------------
// Semantic search
// ---------------------------------------------------------------------------

/// Replaces every chunk belonging to `item_id` with `chunks`, atomically --
/// an edit's re-embed shouldn't leave a window where the item has half its
/// old chunks and half its new ones.
pub async fn replace_chunks(
    pool: &Pool,
    item_id: Uuid,
    user_id: Uuid,
    chunks: &[(String, pgvector::Vector)],
) -> Result<()> {
    let mut client = pool.get().await?;
    let tx = client.transaction().await?;
    tx.execute("DELETE FROM chunks WHERE item_id = $1", &[&item_id])
        .await?;
    for (i, (content, embedding)) in chunks.iter().enumerate() {
        let idx = i as i32;
        tx.execute(
            "INSERT INTO chunks (item_id, user_id, chunk_index, content, embedding)
             VALUES ($1, $2, $3, $4, $5)",
            &[&item_id, &user_id, &idx, content, embedding],
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// One row of what the backfill pass needs: enough to re-derive the text to
/// embed without pulling every column, plus whether it already has chunks so
/// a re-run doesn't redo work that's already done.
pub struct BackfillCandidate {
    pub id: Uuid,
    pub user_id: Uuid,
    pub kind: String,
    pub content_text: Option<String>,
    pub backend: Option<String>,
    pub storage_path: Option<String>,
    pub mime_type: Option<String>,
}

/// One semantic search hit: an item plus how close its nearest chunk was
/// to the query (0 = identical meaning, 2 = opposite -- pgvector's `<=>`
/// is cosine distance).
#[derive(serde::Serialize)]
pub struct SearchHit {
    pub id: Uuid,
    pub kind: String,
    pub content_text: Option<String>,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    pub storage_path: Option<String>,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub distance: f64,
}

/// The bot-side counterpart to the `related_items` RPC the browser calls
/// directly: this one takes an already-embedded query vector (the browser
/// can't compute that itself -- embedding needs the Voyage API key) and
/// scopes by `user_id` explicitly rather than `auth.uid()`, because the bot
/// connects as the `postgres` role with no Supabase JWT in play. `user_id`
/// here always comes from a claimed `semantic_search` token, which already
/// proved ownership via RLS at mint time -- see `claim_write_token`.
pub async fn semantic_search(
    pool: &Pool,
    user_id: Uuid,
    query_embedding: &pgvector::Vector,
    limit: i32,
) -> Result<Vec<SearchHit>> {
    let client = pool.get().await?;
    let rows = client
        .query(
            "SELECT i.id, i.kind, i.content_text, i.file_name, i.mime_type, i.storage_path,
                    i.tags, i.created_at, MIN(c.embedding <=> $2) AS distance
             FROM chunks c
             JOIN items i ON i.id = c.item_id
             WHERE c.user_id = $1
             GROUP BY i.id, i.kind, i.content_text, i.file_name, i.mime_type, i.storage_path,
                      i.tags, i.created_at
             ORDER BY distance ASC
             LIMIT $3",
            &[&user_id, query_embedding, &(limit.clamp(1, 100) as i64)],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| SearchHit {
            id: r.get("id"),
            kind: r.get("kind"),
            content_text: r.get("content_text"),
            file_name: r.get("file_name"),
            mime_type: r.get("mime_type"),
            storage_path: r.get("storage_path"),
            tags: r.get("tags"),
            created_at: r.get("created_at"),
            distance: r.get("distance"),
        })
        .collect())
}

/// Every item that has no chunks yet, across all users -- the backfill
/// command's worklist. Ordered oldest-first so a partial run (rate limits,
/// a restart) makes steady progress rather than looping the same recent
/// items.
pub async fn items_missing_chunks(pool: &Pool) -> Result<Vec<BackfillCandidate>> {
    let client = pool.get().await?;
    let rows = client
        .query(
            "SELECT i.id, i.user_id, i.kind, i.content_text, i.backend, i.storage_path, i.mime_type
             FROM items i
             LEFT JOIN chunks c ON c.item_id = i.id
             WHERE c.id IS NULL
             GROUP BY i.id
             ORDER BY i.created_at ASC",
            &[],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| BackfillCandidate {
            id: r.get("id"),
            user_id: r.get("user_id"),
            kind: r.get("kind"),
            content_text: r.get("content_text"),
            backend: r.get("backend"),
            storage_path: r.get("storage_path"),
            mime_type: r.get("mime_type"),
        })
        .collect())
}
