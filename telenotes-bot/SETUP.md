# Setup, start to finish

Follow this in order. Nothing here costs money; where a free tier has a real
edge you'd hit, it's called out.

Budget about an hour for the first pass. Steps 1–5 get you a working bot;
6–8 are the dashboard and Notion, and can wait.

---

## 0. What you're building

```
   Telegram  ──webhook──▶  bot (Rust, on Render)
                              │
                ┌─────────────┼──────────────┐
                ▼             ▼              ▼
          Supabase       GitHub vault    Cloudflare R2
          Postgres        (markdown       (photos, PDFs,
        (metadata +        notes)          voice, video)
         full-text
          search)
                │
                ▼
      GitHub Pages dashboard
      (reads Postgres directly,
       protected by RLS + magic-link login)
```

Three things to keep straight, because they explain most of the design:

- **Postgres never stores files.** It stores rows describing them. The 500MB
  Supabase limit is for text and metadata, which you will not exhaust.
- **The dashboard is a public web page** even though this repo is private.
  Private Pages is Enterprise-only. So the page ships nothing secret and
  Postgres refuses to answer it until you've logged in.
- **The bot is the only thing holding storage credentials.** The browser never
  sees them.

---

## 1. Create the Telegram bot

1. Open Telegram, message [@BotFather](https://t.me/BotFather).
2. Send `/newbot`. Pick a display name, then a username ending in `bot`.
3. It replies with a token like `123456:ABC-DEF...`. That's `BOT_TOKEN`.
   Treat it like a password — anyone with it controls the bot.

Useful BotFather extras, all optional:

- `/setdescription` — the text shown before someone hits Start.
- `/setuserpic` — an avatar.
- `/setprivacy` → **Disable** — only matters if you add the bot to a group and
  want it to see all messages rather than just commands.

You do **not** need `/setcommands`; the bot registers its own command menu at
startup.

### Find your chat id

You need this for `ALLOWED_CHAT_IDS`, which is what stops strangers using your
bot. Easiest path: deploy first with the variable unset, message the bot, and
it replies with your chat id. Then set the variable and redeploy.

---

## 2. Create the GitHub vault repo

This is a **separate repo** from this one. This repo holds code; the vault
holds captured content.

1. New repo, **private**, e.g. `telenotes-vault`. Add a README so it has one
   commit.
2. GitHub → Settings → Developer settings → **Fine-grained personal access
   tokens** → Generate new token.
   - Repository access: **Only select repositories** → `telenotes-vault`.
   - Permissions: **Contents → Read and write**. Nothing else.
   - Set an expiry you'll actually remember; you'll need to rotate it.
3. Save the token as `GITHUB_TOKEN`, plus `GITHUB_OWNER` (your username) and
   `GITHUB_REPO`.

Notes land as markdown at `notes/YYYY/MM/<id>-<slug>.md`, with YAML
frontmatter. They're readable directly on GitHub and diffable over time.

---

## 3. Create the Supabase project

1. [supabase.com](https://supabase.com) → New project. **Save the database
   password** — it's shown once and you need it for the connection string.
2. SQL Editor → New query → paste all of
   [`migrations/0001_init.sql`](migrations/0001_init.sql) → Run. Then do the
   same with [`migrations/0002_write_api.sql`](migrations/0002_write_api.sql)
   — it adds the dashboard/TUI write API and the analytics RPCs. Both are
   safe to re-run. `0003_fix_gen_random_bytes_search_path.sql` is a forward
   fix for a bug in 0001/0002 — run it too. `0004`/`0005` are for semantic
   search — optional, see [§9](#9-semantic-search-optional).
3. Project Settings → Database → Connection string → the **Session pooler**
   URI. Replace `[YOUR-PASSWORD]` with the password from step 1. That's
   `DATABASE_URL`.

   Use the pooler, not the direct connection. A free Render instance sleeps
   and wakes constantly, and the pooler handles that pattern far better.

4. Project Settings → API → copy the **Project URL** and the **anon public**
   key. You'll need these in step 7.

   The `service_role` key on that same page bypasses Row Level Security
   entirely. It must never go in the web dashboard. The bot doesn't need it
   either — it connects over Postgres directly.

---

## 4. Cloudflare R2 (optional but recommended)

Skip this and files go into the GitHub vault instead; everything still works.
Do it if you'll send photos, voice notes, or anything you might later delete —
git keeps deleted blobs in history forever, R2 doesn't.

Free tier: 10GB stored, 1M writes/month, 10M reads/month, and **no egress
charges at all**.

1. [dash.cloudflare.com](https://dash.cloudflare.com) → R2. You'll be asked
   for a card to activate the free tier; it isn't charged within those limits.
2. Create a bucket, e.g. `telenotes`. Keep it **private** — the dashboard
   reaches files through signed URLs, so public access is never needed.
3. R2 → API → **Manage API tokens** → Create token.
   - Permission: **Object Read & Write**, scoped to that one bucket.
   - Save the **Access Key ID** and **Secret Access Key**.
4. Your **Account ID** is on the R2 overview page.

Set `R2_ACCOUNT_ID`, `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`, `R2_BUCKET`.
All four or none — the bot refuses to start on a half-filled set rather than
silently doing the wrong thing.

---

## 5. Deploy the bot to Render

1. Push this repo to GitHub (already done if you're reading this on a branch).
2. [render.com](https://render.com) → New → **Blueprint** → pick this repo.
   It reads `telenotes-bot/render.yaml`.
3. Plan: **Free**.
4. Fill in the environment variables. `WEBHOOK_SECRET` is generated for you;
   leave `WEBHOOK_URL` blank for now.
5. Deploy. Render assigns a URL like `https://telenotes-bot.onrender.com`.
6. Set `WEBHOOK_URL` to exactly that URL and redeploy. On boot the bot
   registers its own webhook — no manual `setWebhook` call.
7. Message the bot. It replies with your chat id. Put that in
   `ALLOWED_CHAT_IDS` and redeploy one last time.

**Check it worked:** `curl https://your-service.onrender.com/health` returns
`200`. That endpoint runs a real query, so a 200 means Telegram, Render and
Postgres are all wired up.

### If something's wrong

| Symptom | Likely cause |
| --- | --- |
| Bot silent, no logs | `WEBHOOK_URL` doesn't match the real Render URL |
| "This is a private bot" | Your chat id isn't in `ALLOWED_CHAT_IDS` |
| Boot fails on `DATABASE_URL` | Password not substituted, or using the direct connection instead of the pooler |
| Files fail, notes work | GitHub token missing **Contents: Read and write**, or R2 keys wrong |
| First message after idle is slow | Free instance cold start, ~50s. Expected — see below |

---

## 6. Keeping things awake

Two separate clocks, and they need different answers:

**Supabase pauses a free project after 7 days with no database activity.**
Handled already: `.github/workflows/keepalive.yml` pings `/health` daily, which
runs a query. Add a repo secret `HEALTHCHECK_URL` set to
`https://your-service.onrender.com/health` (Settings → Secrets and variables →
Actions). Costs ~30 Actions minutes/month.

**Render free sleeps after 15 minutes idle**, then takes ~50s to wake. Options:

- *Do nothing.* For a personal capture bot this is genuinely fine — you send a
  message, it lands a minute later.
- *Use an external free cron.* [cron-job.org](https://cron-job.org) or
  [UptimeRobot](https://uptimerobot.com) will hit `/health` every 5–10 minutes
  for free. This costs you no Actions minutes, which is why it isn't a
  workflow here — a `*/10` GitHub cron would be ~4,320 minutes/month against
  the 3,000 that Pro/Education includes.

One more thing: GitHub disables scheduled workflows after 60 days of repo
inactivity. If the keepalive goes quiet, that's usually why — push anything,
or re-enable it in the Actions tab.

---

## 7. The web dashboard

1. Edit `web/config.js` with your Supabase URL, anon key, and Render URL.
   All three are safe to commit.
2. Set `DASHBOARD_ORIGIN` on the Render service (step 5's env vars) to this
   page's origin once you know it, e.g. `https://your-username.github.io` —
   no path, no trailing slash. Redeploy the bot after setting it.
3. Repo → Settings → **Pages** → Source: **GitHub Actions**.
4. Push. `.github/workflows/pages.yml` publishes `web/` and prints the URL —
   for a repo named `telenotes` under user `you`, that's
   `https://you.github.io/telenotes/`.
5. In Supabase: **Authentication → URL Configuration**. Set **Site URL** to
   that exact Pages URL, and add it again (with a trailing `**`, e.g.
   `https://you.github.io/telenotes/**`) under **Redirect URLs**.

   Skip this and the magic link email will redirect to Supabase's default
   (`http://localhost:3000`) instead of your real dashboard, which breaks
   sign-in with an `otp_expired`/`access_denied` error — not because the link
   actually expired, but because it redirected somewhere that was never
   listening.
6. Open the dashboard, enter your email, click the emailed link.
7. In Telegram, send `/link`. Paste the code into the dashboard.

That's a one-time pairing. From then on the dashboard shows your notes and
files, with search, tag filtering, downloads — and, from here on, adding
notes, uploading files, editing, and deleting.

**On safety, since the page is publicly reachable:** the only credential it
carries is the anon key, which identifies the project rather than you. The
migration revokes every table permission from the logged-out `anon` role, and
RLS scopes every row to `auth.uid()`. A stranger loading the page gets an
empty list. Every action against the bot — downloading, adding a note,
uploading a file, editing, deleting — works by asking Postgres for a
short-lived, single-use token (which it only issues for rows you own, after
re-checking `auth.uid()` itself) and handing that to the bot. Storage
credentials never reach the browser, and the bot never verifies a Supabase
JWT — the token *is* the authorization, because Postgres already did the real
check before minting it. `DASHBOARD_ORIGIN` is a second, narrower gate: the
bot's `/api/*` write routes only answer CORS preflights from that one origin.

Deleting an item removes the stored file too — the same order (blob, then
row) `/delete` in Telegram already used, now shared code.

---

## 7a. The TUI client (optional)

A terminal app for the same account, for when you're at a laptop rather than
a phone: browse, search, add notes, upload files, edit, delete, and view
stats, from a `ratatui` interface.

1. It needs the same three values as `web/config.js`, as environment
   variables:
   ```bash
   export TELENOTES_SUPABASE_URL=https://your-project-ref.supabase.co
   export TELENOTES_SUPABASE_ANON_KEY=your-anon-public-key
   export TELENOTES_BOT_URL=https://your-service.onrender.com
   ```
   Put these in your shell profile so you don't retype them.
2. **One extra Supabase setting the dashboard doesn't need:** Authentication
   → Email Templates → **Magic Link**, add `{{ .Token }}` somewhere in the
   body. The dashboard signs in via the link in that email; the TUI, having
   no browser to catch a redirect, signs in by having you type the 6-digit
   code instead — but Supabase only includes that code in the email if the
   template asks for it. This doesn't change how the link itself behaves, so
   the dashboard keeps working exactly as before.
3. Build and run it:
   ```bash
   cargo run --release -p telenotes-tui
   ```
   First run: type your email, press Enter, check your inbox, type the code,
   press Enter. The session is cached in `~/.config/telenotes/session.json`
   (mode `600`) and refreshed automatically, so this is a one-time step.
4. Keys, from the main list: `j`/`k` move, `Enter` view, `/` search, `n` add
   a note, `u` upload a file, `e` edit the selected item, `x` delete it
   (press again to confirm), `d` download it, `t` stats, `r` refresh, `q`
   quit.

The TUI talks to Supabase directly for reads (same tables/RPCs, same RLS as
the dashboard) and mints its own write tokens the same way the dashboard
does — there's no separate credential or trust path to keep in sync.

**Using it on another device**: the session cache in step 3 is a local file,
so it doesn't follow you between machines — each device signs in once,
independently. Repeat steps 1–3 there: install Rust (`rustup.rs`) if it isn't
already, clone this repo, set the same three env vars (they're safe to
reuse — the anon key is public by design, see `web/config.js`'s comment),
then `cargo run --release -p telenotes-tui`. No new tokens or Supabase
config needed; every device just becomes another authenticated client of
the same project, same as opening the dashboard in a different browser.

---

## 8. Notion (optional)

Wired but dormant — nothing happens until both variables are set.

1. [notion.so/my-integrations](https://www.notion.so/my-integrations) → New
   integration → internal. Copy the token → `NOTION_TOKEN`.
2. Create a Notion database with exactly these properties:

   | Property | Type |
   | --- | --- |
   | `Name` | Title |
   | `Kind` | Select |
   | `Tags` | Multi-select |
   | `ID` | Text |

3. Open it → `…` menu → **Connections** → add your integration. Easy to
   forget; without it every write 404s.
4. Copy the database id from the URL — the 32-character hex string before the
   `?`. That's `NOTION_DATABASE_ID`.
5. Set both variables and redeploy.

Every capture now also creates a Notion page. It's one-way by design: Postgres
stays the source of truth, so there's no sync conflict to resolve. Mirroring is
best-effort — if Notion is down your note is still saved, with a warning in the
logs.

---

## 9. Semantic search (optional)

Every note and PDF gets chunked and embedded, so Browse can search by meaning
("papers about attention mechanisms") instead of exact keywords, and each
item card gets a "Related" button showing other items about the same thing.
Skip this and everything else works exactly as before — it's additive.

1. SQL Editor → run
   [`migrations/0004_semantic_search.sql`](migrations/0004_semantic_search.sql),
   then [`migrations/0005_search_token.sql`](migrations/0005_search_token.sql),
   then [`migrations/0006_chunks_select_grant.sql`](migrations/0006_chunks_select_grant.sql)
   (0004 alone leaves the "Related" button erroring with "permission denied
   for table chunks" — it revoked `chunks` access from `authenticated` but
   never granted it back). All three are safe to re-run.
2. [voyageai.com](https://www.voyageai.com) → sign up → API keys. The free
   tier is 200M tokens, which is a lot of notes and papers. Copy the key →
   `VOYAGE_API_KEY`.
3. Set it on Render and redeploy.
4. Backfill existing items once, so everything you captured before today is
   searchable too — open a shell on the Render service (or run locally with
   the same `DATABASE_URL`/`VOYAGE_API_KEY`) and run:

   ```bash
   telenotes-bot --backfill
   ```

   This walks every item that has no chunks yet, extracts text (the note body,
   or the text layer of a PDF), embeds it, and exits. Safe to re-run — it
   only ever processes items still missing chunks. Scanned PDFs with no text
   layer are skipped; everything else still saves normally, just without
   embeddings.

From here on, new notes/files/edits are embedded automatically in the
background — best-effort, same philosophy as the Notion mirror: if Voyage is
briefly down, your item still saves, just without embeddings for the moment.

---

## Limits worth knowing

| Thing | Limit | What happens |
| --- | --- | --- |
| Telegram file download | **20MB** | Hard bot API limit. The bot tells you and skips it |
| GitHub Contents API | ~45MB/file | Rejected with a clear error |
| GitHub vault repo | keep under a few GB | Deleted blobs stay in history |
| Supabase free DB | 500MB | Metadata only — you won't hit it |
| Supabase pause | 7 days idle | Daily keepalive prevents it |
| R2 free | 10GB, 1M writes, 10M reads/mo | Ample for personal use |
| Render free | sleeps at 15 min idle | ~50s cold start |
| Actions (private repo) | 3,000 min/mo on Pro | Why keepalive is daily, not `*/10` |

---

## Running locally

```bash
cd telenotes-bot
cp .env.example .env      # fill it in
cargo run
```

Telegram needs a public HTTPS URL for webhooks, so for local work expose the
port with something like [ngrok](https://ngrok.com) and set `WEBHOOK_URL` to
the forwarding address.

```bash
cargo test          # unit tests
cargo clippy --all-targets -- -D warnings
cargo fmt
```

## Rotating credentials

If a token leaks, replace it and redeploy:

- **Bot token** — BotFather → `/revoke`, then `/token`.
- **GitHub PAT** — delete and regenerate; the vault contents are unaffected.
- **R2 keys** — roll the API token in the Cloudflare dashboard.
- **`WEBHOOK_SECRET`** — change it in Render; the bot re-registers on boot.
- **Supabase anon key** — only rotate if you suspect the project itself is
  compromised; the anon key is public by design.
