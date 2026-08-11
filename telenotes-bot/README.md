# telenotes-bot

A Telegram bot that captures notes and files from your phone into storage you
own, with full-text search and a web dashboard. Built to run entirely on free
tiers.

Send it text and it's a searchable note. Send it a PDF, photo, voice memo or
video and it's archived. Tag anything with `#like-this`.

**Setup instructions: [SETUP.md](SETUP.md).**

## Stack

| Piece | What it does |
| --- | --- |
| Rust + [teloxide](https://github.com/teloxide/teloxide) / [axum](https://github.com/tokio-rs/axum) | The bot, in webhook mode |
| [Supabase](https://supabase.com) Postgres | Metadata, tags, full-text search |
| Supabase Auth + RLS | Login and row-level access control for the dashboard |
| Private GitHub repo | Notes, stored as markdown |
| [Cloudflare R2](https://developers.cloudflare.com/r2/) *(optional)* | Binary files |
| [Render](https://render.com) free web service | Hosting |
| GitHub Pages | The dashboard (`../web`) |

## Why storage is split

Notes go to GitHub because markdown in git is exactly right for text you want
to keep: readable, diffable, versioned, greppable.

Files go to R2 because git is exactly wrong for binaries. Every upload is a
commit, and deleting a file only removes it from the branch tip — the blob
stays in history and keeps counting against the repo forever. R2 has no such
problem, gives 10GB free, and charges nothing for egress.

R2 is optional. Leave the `R2_*` variables unset and files land in the GitHub
vault instead; everything still works.

## Commands

| Command | Effect |
| --- | --- |
| *(any text)* | Saved as a note, `#tags` extracted |
| *(any file)* | Archived; the caption becomes searchable text |
| `/notes [tag]` | Recent notes, optionally filtered |
| `/files [tag]` | Recent files |
| `/search <query>` | Full-text search over notes, captions and filenames |
| `/tags` | Tags in use, with counts |
| `/get <id>` | Resend a note or file |
| `/delete <id>` | Delete an item **and** its stored file |
| `/stats` | Counts and storage in use |
| `/link` | Code to pair the web dashboard |
| `/help` | The list above |

Ids are the 8-character prefix shown in listings.

## Layout

```
src/
  main.rs        HTTP surface: webhook, health, download tokens
  config.rs      environment parsing, fails fast and loudly
  handlers.rs    message and command handling
  db.rs          queries
  models.rs      row types
  notion.rs      optional one-way Notion mirror
  storage/
    mod.rs       Vault: routes notes to GitHub, files to R2
    github.rs    Contents API
    r2.rs        S3-compatible, presigned URLs only
migrations/
  0001_init.sql  schema, RLS policies, RPCs
```

## Design notes

**Webhook authentication.** The secret is registered with Telegram via
`set_webhook` and checked against the `X-Telegram-Bot-Api-Secret-Token` header
on every delivery, rather than sitting in the URL path where it would leak into
logs.

**Access control.** `ALLOWED_CHAT_IDS` is checked before any database write, so
an unknown chat can't even create a user row. Leave it unset and the bot warns
at startup.

**Write ordering.** For files, bytes are written to storage before the row —
an orphaned blob is easy to clean up, but a row pointing at a file that was
never written is broken forever. For notes it's reversed: Postgres holds the
full text and is the source of truth, so the markdown mirror is best-effort.

**Deletes.** `/delete` removes the blob first and only then the row, so a
storage failure leaves the item intact and retryable.

**Resends.** `/get` tries Telegram's cached `file_id` first, which is instant
and costs no storage bandwidth, and falls back to the vault only if that's
expired.

**Downloads from the dashboard.** The browser asks Postgres for a 5-minute
capability token — issued only for rows you own, enforced by RLS — and hands
it to the bot, which redirects to a presigned R2 URL. Storage credentials
never leave the server, and the bot never has to verify a JWT.

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt
```

CI runs all three on any change under `telenotes-bot/`.
