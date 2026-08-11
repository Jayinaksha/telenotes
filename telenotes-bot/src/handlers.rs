use anyhow::Result;
use chrono::Datelike;
use regex::Regex;
use std::sync::OnceLock;
use teloxide::prelude::*;
use teloxide::types::{FileId, FileMeta, InputFile, MessageKind, Update, UpdateKind};
use uuid::Uuid;

use crate::db::{self, NewFile};
use crate::models::Item;
use crate::notion::NotionPage;
use crate::AppState;

/// Telegram's Bot API only serves `getFile` downloads up to 20MB. Anything
/// larger we have to refuse explicitly -- the bot literally cannot fetch the
/// bytes, whatever storage backend is behind it.
const TELEGRAM_DOWNLOAD_LIMIT: i64 = 20 * 1024 * 1024;

const LIST_LIMIT: i64 = 20;

fn tag_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Compiled once: rebuilding this per message was measurable overhead and
    // the old code also unwrapped on every call.
    RE.get_or_init(|| Regex::new(r"#(\w+)").expect("tag regex is valid"))
}

pub async fn handle_update(state: AppState, update: Update) {
    if let UpdateKind::Message(msg) = update.kind {
        if let Err(e) = handle_message(&state, msg).await {
            tracing::error!("error handling message: {e:?}");
        }
    }
}

fn extract_tags(text: &str) -> Vec<String> {
    let mut tags: Vec<String> = tag_regex()
        .captures_iter(text)
        .map(|c| c[1].to_lowercase())
        .collect();
    tags.sort();
    tags.dedup();
    tags
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

/// Turns a note's first line into a filesystem-safe slug for the vault.
fn slugify(text: &str) -> String {
    let slug: String = text
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .take(8)
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        "note".to_string()
    } else {
        slug
    }
}

/// Postgres is the source of truth for a note, so the row is written first and
/// the markdown mirror is best-effort. Losing the mirror is recoverable;
/// losing the note because GitHub was briefly unavailable is not.
async fn save_note(state: &AppState, chat_id: ChatId, user_id: Uuid, text: &str) -> Result<()> {
    let id = Uuid::new_v4();
    let short = &id.to_string()[..8];
    let tags = extract_tags(text);
    let now = chrono::Utc::now();

    let path = format!(
        "notes/{:04}/{:02}/{}-{}.md",
        now.year(),
        now.month(),
        short,
        slugify(text)
    );

    db::insert_note(&state.db, id, user_id, text, &tags, &path).await?;

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

    mirror_to_notion(
        state,
        NotionPage {
            title: text.lines().next().unwrap_or("note"),
            kind: "note",
            body: Some(text),
            tags: &tags,
            short_id: short,
        },
    )
    .await;

    let suffix = if tags.is_empty() {
        String::new()
    } else {
        format!(
            " · {}",
            tags.iter()
                .map(|t| format!("#{t}"))
                .collect::<Vec<_>>()
                .join(" ")
        )
    };
    state
        .bot
        .send_message(chat_id, format!("Saved note {short}{suffix}"))
        .await
        .ok();
    Ok(())
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

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
    let content_type = mime_type.unwrap_or("application/octet-stream");

    // Bytes first: an orphaned blob is cheap to clean up, but a row pointing
    // at a file that was never written is a broken item forever.
    let backend = state
        .vault
        .put_file(
            &storage_path,
            bytes,
            content_type,
            &format!("Add {kind}: {name}"),
        )
        .await?;

    let tags = caption.map(extract_tags).unwrap_or_default();
    let id = db::insert_file(
        &state.db,
        user_id,
        NewFile {
            kind,
            content_text: caption,
            file_id: &meta.id.0,
            file_unique_id: &meta.unique_id.0,
            file_name: Some(&name),
            mime_type,
            file_size: Some(size),
            backend,
            storage_path: &storage_path,
            tags: &tags,
        },
    )
    .await?;

    let short = &id.to_string()[..8];
    mirror_to_notion(
        state,
        NotionPage {
            title: &name,
            kind,
            body: caption,
            tags: &tags,
            short_id: short,
        },
    )
    .await;

    state
        .bot
        .send_message(
            chat_id,
            format!("Saved {kind} {short} — {name} ({})", backend.as_str()),
        )
        .await
        .ok();
    Ok(())
}

/// Keeps a Telegram-supplied filename from escaping its directory or
/// upsetting object-store key rules.
fn sanitize(name: &str) -> String {
    // Everything outside the safe set collapses to a single underscore, then
    // leading/trailing separators and dots go. That turns "../../etc/passwd"
    // into "etc_passwd" rather than a technically-safe but ugly "_.._etc...".
    let mut out = String::new();
    for c in name.chars() {
        match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' => out.push(c),
            _ if !out.ends_with('_') && !out.is_empty() => out.push('_'),
            _ => {}
        }
    }

    let trimmed = out.trim_matches(|c| c == '.' || c == '_' || c == '-');
    if trimmed.is_empty() {
        "file".to_string()
    } else {
        trimmed.chars().take(120).collect()
    }
}

async fn mirror_to_notion(state: &AppState, page: NotionPage<'_>) {
    let Some(notion) = &state.notion else { return };
    if let Err(e) = notion.create_page(page).await {
        tracing::warn!("notion mirror failed: {e:?}");
    }
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
            Some(item) => {
                // Remove the bytes before the row, so a storage failure leaves
                // the item intact and retryable rather than orphaning a blob.
                if let Some(path) = &item.storage_path {
                    if let Err(e) = state
                        .vault
                        .delete(item.backend(), path, &format!("delete {}", item.short_id()))
                        .await
                    {
                        tracing::error!("failed to delete blob for {}: {e:?}", item.short_id());
                        state
                            .bot
                            .send_message(
                                chat_id,
                                "Couldn't remove the stored file; nothing deleted.",
                            )
                            .await?;
                        return Ok(());
                    }
                }
                db::delete_item(&state.db, user_id, item.id).await?;
                state
                    .bot
                    .send_message(chat_id, format!("Deleted {}.", item.short_id()))
                    .await?;
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_lowercased_sorted_and_deduped() {
        assert_eq!(
            extract_tags("a #Radar note about #SAR and #radar again"),
            vec!["radar".to_string(), "sar".to_string()]
        );
    }

    #[test]
    fn text_without_tags_yields_none() {
        assert!(extract_tags("no tags here").is_empty());
    }

    #[test]
    fn slug_uses_first_line_and_bounds_length() {
        assert_eq!(
            slugify("Hello World: a test\nsecond line"),
            "hello-world-a-test"
        );
        assert_eq!(slugify(""), "note");
        assert_eq!(slugify("!!!"), "note");
        assert_eq!(
            slugify("one two three four five six seven eight nine")
                .split('-')
                .count(),
            8
        );
    }

    #[test]
    fn sanitize_strips_path_traversal() {
        assert_eq!(sanitize("../../etc/passwd"), "etc_passwd");
        assert_eq!(sanitize("report final.pdf"), "report_final.pdf");
        assert_eq!(sanitize(""), "file");
        assert_eq!(sanitize("..."), "file");
    }
}
