use anyhow::{Context, Result};
use chrono::Datelike;
use telenotes_core::text::{extract_tags, sanitize, slugify};
use telenotes_core::Item;
use teloxide::prelude::*;
use teloxide::types::{FileId, FileMeta, InputFile, MessageKind, Update, UpdateKind};
use uuid::Uuid;

use crate::db::{self, NewFile};
use crate::notion::NotionPage;
use crate::AppState;

/// Telegram's Bot API only serves `getFile` downloads up to 20MB. Anything
/// larger we have to refuse explicitly -- the bot literally cannot fetch the
/// bytes, whatever storage backend is behind it.
const TELEGRAM_DOWNLOAD_LIMIT: i64 = 20 * 1024 * 1024;

const LIST_LIMIT: i64 = 20;

pub async fn handle_update(state: AppState, update: Update) {
    if let UpdateKind::Message(msg) = update.kind {
        if let Err(e) = handle_message(&state, msg).await {
            tracing::error!("error handling message: {e:?}");
        }
    }
}

async fn handle_message(state: &AppState, msg: Message) -> Result<()> {
    if !matches!(msg.kind, MessageKind::Common(_)) {
        return Ok(());
    }

    let chat_id = msg.chat.id;

    // Gate before touching the database, so an unknown chat can't even create
    // a user row. Without this anyone who discovers the bot's handle can write
    // into your vault and burn your storage quota.
    if !state.config.is_allowed(chat_id.0) {
        tracing::warn!("rejected message from chat {}", chat_id.0);
        state
            .bot
            .send_message(
                chat_id,
                format!(
                    "This is a private bot.\n\nYour chat id is {}. If it's yours, \
                     add it to ALLOWED_CHAT_IDS and redeploy.",
                    chat_id.0
                ),
            )
            .await
            .ok();
        return Ok(());
    }

    let username = msg.from.as_ref().and_then(|u| u.username.as_deref());
    let user_id = db::ensure_user(&state.db, chat_id.0, username).await?;

    if let Some(text) = msg.text() {
        if let Some(rest) = text.strip_prefix('/') {
            return handle_command(state, chat_id, user_id, rest).await;
        }
        return save_note(state, chat_id, user_id, text).await;
    }

    // Files. `FileMeta` is the shared shape across every attachment type, so
    // each arm just names the kind and any type-specific metadata.
    let attachment: Option<(&str, &FileMeta, Option<&str>, Option<String>)> =
        if let Some(photo) = msg.photo().and_then(|p| p.last()) {
            // Photos arrive as a ladder of sizes; the last is the largest.
            Some(("photo", &photo.file, None, Some("image/jpeg".to_string())))
        } else if let Some(doc) = msg.document() {
            Some((
                "document",
                &doc.file,
                doc.file_name.as_deref(),
                doc.mime_type.as_ref().map(|m| m.to_string()),
            ))
        } else if let Some(voice) = msg.voice() {
            Some((
                "voice",
                &voice.file,
                None,
                voice.mime_type.as_ref().map(|m| m.to_string()),
            ))
        } else if let Some(video) = msg.video() {
            Some((
                "video",
                &video.file,
                video.file_name.as_deref(),
                video.mime_type.as_ref().map(|m| m.to_string()),
            ))
        } else if let Some(audio) = msg.audio() {
            Some((
                "audio",
                &audio.file,
                audio.file_name.as_deref(),
                audio.mime_type.as_ref().map(|m| m.to_string()),
            ))
        } else {
            None
        };

    if let Some((kind, meta, file_name, mime_type)) = attachment {
        save_incoming_file(
            state,
            chat_id,
            user_id,
            kind,
            meta,
            file_name,
            mime_type.as_deref(),
            msg.caption(),
        )
        .await?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Notes
// ---------------------------------------------------------------------------

/// Postgres is the source of truth for a note, so the row is written first and
/// the markdown mirror is best-effort. Losing the mirror is recoverable;
/// losing the note because GitHub was briefly unavailable is not.
///
/// Shared by the Telegram path (`save_note`) and the dashboard/TUI write API
/// (`api::create_note_handler`).
pub(crate) async fn create_note(state: &AppState, user_id: Uuid, text: &str) -> Result<Item> {
    let id = Uuid::new_v4();
    let short = id.to_string()[..8].to_string();
    let tags = extract_tags(text);
    let now = chrono::Utc::now();

    let path = format!(
        "notes/{:04}/{:02}/{}-{}.md",
        now.year(),
        now.month(),
        short,
        slugify(text)
    );

    let created_at = db::insert_note(&state.db, id, user_id, text, &tags, &path).await?;

    let frontmatter = format!(
        "---\nid: {id}\ncreated: {}\ntags: [{}]\n---\n\n{text}\n",
        now.to_rfc3339(),
        tags.join(", ")
    );
    if let Err(e) = state
        .vault
        .put_note(&path, &frontmatter, &format!("note: {}", slugify(text)))
        .await
    {
        tracing::warn!("note {short} saved to Postgres but not mirrored to GitHub: {e:?}");
    }

    if let Some(page_id) = mirror_to_notion(
        state,
        NotionPage {
            title: text.lines().next().unwrap_or("note"),
            kind: "note",
            body: Some(text),
            tags: &tags,
            short_id: &short,
        },
    )
    .await
    {
        if let Err(e) = db::set_notion_page_id(&state.db, id, &page_id).await {
            tracing::warn!("note {short} mirrored to Notion but page id not saved: {e:?}");
        }
    }

    // Chunking + embedding happen off to the side: they're pure value-add
    // for semantic search, never something a note's save should wait on or
    // fail because of.
    spawn_embedding(state, id, user_id, text.to_string());

    Ok(Item {
        id,
        user_id,
        kind: "note".to_string(),
        content_text: Some(text.to_string()),
        file_id: None,
        file_unique_id: None,
        backend: Some("github".to_string()),
        storage_path: Some(path),
        file_name: None,
        mime_type: None,
        file_size: None,
        tags,
        created_at,
    })
}

async fn save_note(state: &AppState, chat_id: ChatId, user_id: Uuid, text: &str) -> Result<()> {
    let item = create_note(state, user_id, text).await?;
    let suffix = if item.tags.is_empty() {
        String::new()
    } else {
        format!(
            " · {}",
            item.tags
                .iter()
                .map(|t| format!("#{t}"))
                .collect::<Vec<_>>()
                .join(" ")
        )
    };
    state
        .bot
        .send_message(chat_id, format!("Saved note {}{suffix}", item.short_id()))
        .await
        .ok();
    Ok(())
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

/// Shared by the Telegram path (`save_incoming_file`) and the dashboard/TUI
/// write API (`api::upload_file_handler`). `file_id`/`file_unique_id` are
/// `None` for uploads with no Telegram provenance.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_file(
    state: &AppState,
    user_id: Uuid,
    kind: &str,
    storage_path: &str,
    bytes: Vec<u8>,
    file_name: &str,
    mime_type: Option<&str>,
    caption: Option<&str>,
    file_id: Option<&str>,
    file_unique_id: Option<&str>,
) -> Result<Item> {
    let size = bytes.len() as i64;
    let content_type = mime_type.unwrap_or("application/octet-stream");

    // Extracted before the upload below consumes `bytes` -- best-effort:
    // a PDF with no text layer (scanned images) or one pdf-extract can't
    // parse just means semantic search won't cover it, the file itself is
    // unaffected either way.
    let pdf_text = if content_type == "application/pdf" {
        match crate::pdf::extract_text(&bytes) {
            Ok(t) if !t.trim().is_empty() => Some(t),
            Ok(_) => None,
            Err(e) => {
                tracing::warn!("PDF text extraction failed for {file_name}: {e:?}");
                None
            }
        }
    } else {
        None
    };

    // Bytes first: an orphaned blob is cheap to clean up, but a row pointing
    // at a file that was never written is a broken item forever.
    let backend = state
        .vault
        .put_file(
            storage_path,
            bytes,
            content_type,
            &format!("Add {kind}: {file_name}"),
        )
        .await?;

    let tags = caption.map(extract_tags).unwrap_or_default();
    let (id, created_at) = db::insert_file(
        &state.db,
        user_id,
        NewFile {
            kind,
            content_text: caption,
            file_id,
            file_unique_id,
            file_name: Some(file_name),
            mime_type,
            file_size: Some(size),
            backend,
            storage_path,
            tags: &tags,
        },
    )
    .await?;

    let short = id.to_string()[..8].to_string();
    if let Some(page_id) = mirror_to_notion(
        state,
        NotionPage {
            title: file_name,
            kind,
            body: caption,
            tags: &tags,
            short_id: &short,
        },
    )
    .await
    {
        if let Err(e) = db::set_notion_page_id(&state.db, id, &page_id).await {
            tracing::warn!("file {short} mirrored to Notion but page id not saved: {e:?}");
        }
    }

    // Embed the PDF's own text if we extracted any; otherwise fall back to
    // the caption, so a photo or voice memo with a written note attached is
    // still findable by meaning even without OCR/transcription.
    if let Some(text) = pdf_text.or_else(|| caption.map(str::to_string)) {
        spawn_embedding(state, id, user_id, text);
    }

    Ok(Item {
        id,
        user_id,
        kind: kind.to_string(),
        content_text: caption.map(str::to_string),
        file_id: file_id.map(str::to_string),
        file_unique_id: file_unique_id.map(str::to_string),
        backend: Some(backend.as_str().to_string()),
        storage_path: Some(storage_path.to_string()),
        file_name: Some(file_name.to_string()),
        mime_type: mime_type.map(str::to_string),
        file_size: Some(size),
        tags,
        created_at,
    })
}

#[allow(clippy::too_many_arguments)]
async fn save_incoming_file(
    state: &AppState,
    chat_id: ChatId,
    user_id: Uuid,
    kind: &str,
    meta: &FileMeta,
    file_name: Option<&str>,
    mime_type: Option<&str>,
    caption: Option<&str>,
) -> Result<()> {
    let size = meta.size as i64;
    if size > TELEGRAM_DOWNLOAD_LIMIT {
        state
            .bot
            .send_message(
                chat_id,
                format!(
                    "That file is {}MB. Telegram only lets bots download files up to 20MB, \
                     so I can't archive it.",
                    size / 1024 / 1024
                ),
            )
            .await
            .ok();
        return Ok(());
    }

    let tg_file = state.bot.get_file(meta.id.clone()).await?;
    let download_url = format!(
        "https://api.telegram.org/file/bot{}/{}",
        state.config.bot_token, tg_file.path
    );
    let bytes = state
        .http
        .get(download_url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?
        .to_vec();

    let name = file_name
        .map(str::to_string)
        .unwrap_or_else(|| format!("{kind}_{}", meta.unique_id.0));
    let storage_path = format!("files/{user_id}/{}_{}", meta.unique_id.0, sanitize(&name));

    let item = create_file(
        state,
        user_id,
        kind,
        &storage_path,
        bytes,
        &name,
        mime_type,
        caption,
        Some(&meta.id.0),
        Some(&meta.unique_id.0),
    )
    .await?;

    state
        .bot
        .send_message(
            chat_id,
            format!(
                "Saved {kind} {} — {name} ({})",
                item.short_id(),
                item.backend.as_deref().unwrap_or("github")
            ),
        )
        .await
        .ok();
    Ok(())
}

/// Chunks and embeds `text` in the background, off the request/message path.
/// A no-op if `VOYAGE_API_KEY` isn't set -- `embed_item` checks that itself.
fn spawn_embedding(state: &AppState, item_id: Uuid, user_id: Uuid, text: String) {
    let state = state.clone();
    tokio::spawn(async move {
        crate::embeddings::embed_item(&state, item_id, user_id, &text).await;
    });
}

/// Returns the new page's id on success, so callers can store it on the
/// item's `notion_page_id` column for a later edit/delete to find again.
async fn mirror_to_notion(state: &AppState, page: NotionPage<'_>) -> Option<String> {
    let notion = state.notion.as_ref()?;
    match notion.create_page(page).await {
        Ok(page_id) => Some(page_id),
        Err(e) => {
            tracing::warn!("notion mirror failed: {e:?}");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Edit and delete
// ---------------------------------------------------------------------------

/// Shared by the Telegram `/delete` command and `api::edit_item_handler`.
/// `None` in any optional field leaves that column unchanged. `Ok(None)`
/// means no such item for this user; a real failure is an `Err`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn edit_item(
    state: &AppState,
    user_id: Uuid,
    item_id: Uuid,
    content_text: Option<&str>,
    tags: Option<&[String]>,
    file_name: Option<&str>,
    mime_type: Option<&str>,
) -> Result<Option<Item>> {
    let Some(item) = db::update_item(
        &state.db,
        item_id,
        user_id,
        content_text,
        tags,
        file_name,
        mime_type,
    )
    .await?
    else {
        return Ok(None);
    };

    // Mirror sync is best-effort: Postgres already holds the authoritative
    // edit, so a mirror failure here must never undo it, only be logged.
    if item.kind == "note" && (content_text.is_some() || tags.is_some()) {
        if let Some(path) = &item.storage_path {
            let text = item.content_text.clone().unwrap_or_default();
            let frontmatter = format!(
                "---\nid: {}\ncreated: {}\ntags: [{}]\n---\n\n{text}\n",
                item.id,
                item.created_at.to_rfc3339(),
                item.tags.join(", ")
            );
            if let Err(e) = state
                .vault
                .put_note(path, &frontmatter, &format!("edit: {}", item.short_id()))
                .await
            {
                tracing::warn!(
                    "note {} edited in Postgres but not remirrored to GitHub: {e:?}",
                    item.short_id()
                );
            }
        }
    }

    if let Some(notion) = &state.notion {
        match db::get_notion_page_id(&state.db, item.id).await {
            Ok(Some(page_id)) => {
                let short = item.short_id();
                let title = if item.kind == "note" {
                    item.content_text
                        .as_deref()
                        .and_then(|t| t.lines().next())
                        .unwrap_or("note")
                        .to_string()
                } else {
                    item.file_name
                        .clone()
                        .unwrap_or_else(|| format!("{} file", item.kind))
                };
                let page = NotionPage {
                    title: &title,
                    kind: &item.kind,
                    body: item.content_text.as_deref(),
                    tags: &item.tags,
                    short_id: &short,
                };
                if let Err(e) = notion
                    .update_page(&page_id, page, content_text.is_some())
                    .await
                {
                    tracing::warn!("item {short} edited but Notion mirror update failed: {e:?}");
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(
                "could not look up Notion page id for {}: {e:?}",
                item.short_id()
            ),
        }
    }

    // Re-embed on a text change so semantic search reflects the edit rather
    // than the item's original wording. Skipped for files whose caption
    // didn't change -- a filename/mime-type-only edit has no new text, and
    // re-extracting a PDF's text layer on every metadata tweak would be
    // wasted work for no different embedding.
    if content_text.is_some() {
        if let Some(text) = item.content_text.clone() {
            spawn_embedding(state, item.id, user_id, text);
        }
    }

    Ok(Some(item))
}

/// Shared by the Telegram `/delete` command and `api::delete_item_handler`.
/// Removes the bytes before the row, so a storage failure leaves the item
/// intact and retryable rather than orphaning a blob. `Ok(None)` means no
/// such item for this user; a storage/db failure is an `Err`.
pub(crate) async fn delete_item(
    state: &AppState,
    user_id: Uuid,
    item_id: Uuid,
) -> Result<Option<Item>> {
    let Some(item) = db::get_item(&state.db, item_id).await? else {
        return Ok(None);
    };
    if item.user_id != user_id {
        return Ok(None);
    }

    if let Some(path) = &item.storage_path {
        state
            .vault
            .delete(item.backend(), path, &format!("delete {}", item.short_id()))
            .await
            .with_context(|| format!("couldn't remove the stored file for {}", item.short_id()))?;
    }

    let notion_page_id = db::get_notion_page_id(&state.db, item.id)
        .await
        .ok()
        .flatten();

    db::delete_item(&state.db, user_id, item.id).await?;

    if let (Some(notion), Some(page_id)) = (&state.notion, &notion_page_id) {
        if let Err(e) = notion.archive_page(page_id).await {
            tracing::warn!(
                "item {} deleted but Notion mirror archive failed: {e:?}",
                item.short_id()
            );
        }
    }

    Ok(Some(item))
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

async fn handle_command(
    state: &AppState,
    chat_id: ChatId,
    user_id: Uuid,
    rest: &str,
) -> Result<()> {
    let mut parts = rest.splitn(2, ' ');
    // Telegram appends @botname in groups; strip it so commands work there.
    let raw_cmd = parts.next().unwrap_or("");
    let cmd = raw_cmd.split('@').next().unwrap_or("").to_lowercase();
    let arg = parts.next().unwrap_or("").trim();
    let tag = (!arg.is_empty()).then_some(arg);

    match cmd.as_str() {
        "start" | "help" => {
            state.bot.send_message(chat_id, HELP_TEXT).await?;
        }
        "notes" => {
            let items = db::list_items(&state.db, user_id, Some("note"), tag, LIST_LIMIT).await?;
            reply_with_list(state, chat_id, "Notes", &items).await?;
        }
        "files" => {
            let items = db::list_files(&state.db, user_id, tag, LIST_LIMIT).await?;
            reply_with_list(state, chat_id, "Files", &items).await?;
        }
        "search" => {
            if arg.is_empty() {
                state
                    .bot
                    .send_message(chat_id, "Usage: /search <query>")
                    .await?;
            } else {
                let items = db::search_items(&state.db, user_id, arg, LIST_LIMIT).await?;
                reply_with_list(state, chat_id, &format!("Results for \"{arg}\""), &items).await?;
            }
        }
        "tags" => {
            let tags = db::list_tags(&state.db, user_id).await?;
            let text = if tags.is_empty() {
                "No tags yet. Add #tags to notes or captions.".to_string()
            } else {
                tags.iter()
                    .map(|(t, n)| format!("#{t} ({n})"))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            state.bot.send_message(chat_id, text).await?;
        }
        "get" => match db::get_item_by_short_id(&state.db, user_id, arg).await? {
            Some(item) => send_item(state, chat_id, &item).await?,
            None => {
                state
                    .bot
                    .send_message(chat_id, "Not found. Usage: /get <id from a listing>")
                    .await?;
            }
        },
        "delete" => match db::get_item_by_short_id(&state.db, user_id, arg).await? {
            Some(item) => match delete_item(state, user_id, item.id).await {
                Ok(Some(deleted)) => {
                    state
                        .bot
                        .send_message(chat_id, format!("Deleted {}.", deleted.short_id()))
                        .await?;
                }
                Ok(None) => {
                    state
                        .bot
                        .send_message(chat_id, "Not found. Usage: /delete <id from a listing>")
                        .await?;
                }
                Err(e) => {
                    tracing::error!("failed to delete {}: {e:?}", item.short_id());
                    state
                        .bot
                        .send_message(chat_id, "Couldn't remove the stored file; nothing deleted.")
                        .await?;
                }
            },
            None => {
                state
                    .bot
                    .send_message(chat_id, "Not found. Usage: /delete <id from a listing>")
                    .await?;
            }
        },
        "link" => {
            let (code, minutes) = db::create_link_code(&state.db, user_id).await?;
            state
                .bot
                .send_message(
                    chat_id,
                    format!(
                        "Your linking code is:\n\n{code}\n\nSign in on the web dashboard, \
                         then paste this code to connect it to this Telegram account. \
                         It expires in {minutes} minutes and can be used once."
                    ),
                )
                .await?;
        }
        "stats" => {
            let s = db::stats(&state.db, user_id).await?;
            let storage = if state.vault.has_r2() {
                "GitHub (notes) + R2 (files)"
            } else {
                "GitHub only"
            };
            state
                .bot
                .send_message(
                    chat_id,
                    format!(
                        "{} notes\n{} files ({:.1} MB)\n{} tags\n\nStorage: {storage}",
                        s.notes,
                        s.files,
                        s.bytes_stored as f64 / 1024.0 / 1024.0,
                        s.tags,
                    ),
                )
                .await?;
        }
        _ => {
            state
                .bot
                .send_message(chat_id, "Unknown command. /help for the list.")
                .await?;
        }
    }
    Ok(())
}

async fn reply_with_list(
    state: &AppState,
    chat_id: ChatId,
    title: &str,
    items: &[Item],
) -> Result<()> {
    if items.is_empty() {
        state
            .bot
            .send_message(chat_id, format!("{title}: nothing yet."))
            .await?;
        return Ok(());
    }
    let body = items
        .iter()
        .map(Item::summary)
        .collect::<Vec<_>>()
        .join("\n");
    state
        .bot
        .send_message(
            chat_id,
            format!("{title}\n\n{body}\n\nUse /get <id> to fetch one."),
        )
        .await?;
    Ok(())
}

/// Resends a saved item. Files try Telegram's own cached `file_id` first,
/// which is instant and costs no storage bandwidth; only if that has expired
/// do we pull the bytes back out of the vault.
async fn send_item(state: &AppState, chat_id: ChatId, item: &Item) -> Result<()> {
    if item.kind == "note" {
        let text = item.content_text.clone().unwrap_or_default();
        state.bot.send_message(chat_id, text).await?;
        return Ok(());
    }

    if let Some(file_id) = &item.file_id {
        let input = InputFile::file_id(FileId(file_id.clone()));
        let sent = match item.kind.as_str() {
            "photo" => state.bot.send_photo(chat_id, input).await.map(|_| ()),
            "video" => state.bot.send_video(chat_id, input).await.map(|_| ()),
            "voice" => state.bot.send_voice(chat_id, input).await.map(|_| ()),
            "audio" => state.bot.send_audio(chat_id, input).await.map(|_| ()),
            _ => state.bot.send_document(chat_id, input).await.map(|_| ()),
        };
        if sent.is_ok() {
            return Ok(());
        }
        tracing::info!(
            "file_id resend failed for {}, falling back to vault",
            item.short_id()
        );
    }

    let Some(path) = &item.storage_path else {
        state
            .bot
            .send_message(chat_id, "This item has no retrievable file.")
            .await?;
        return Ok(());
    };

    let bytes = state.vault.get(item.backend(), path).await?;
    let name = item.file_name.clone().unwrap_or_else(|| "file".to_string());
    state
        .bot
        .send_document(chat_id, InputFile::memory(bytes).file_name(name))
        .await?;
    Ok(())
}

pub const HELP_TEXT: &str = "Send me any text and I'll save it as a searchable note. \
Send a document, photo, voice note, video, or audio file and I'll archive it. \
Add #tags anywhere in the text or caption to organise things.

Commands:
/notes [tag] - list your notes
/files [tag] - list your stored files
/search <query> - full-text search notes, captions and filenames
/tags - list tags in use
/get <id> - resend a note or file
/delete <id> - delete an item and its stored file
/stats - what you've captured so far
/link - get a code to sign in to the web dashboard
/help - this message";
