# telenotes

Capture notes and files from your phone into storage you own, via Telegram,
the web, or a terminal.

- **[telenotes-bot/](telenotes-bot/)** — the bot (Rust, webhook mode), and
  its `/api/*` write endpoints used by the dashboard and TUI. Start with its
  [README](telenotes-bot/README.md) for what it does, or go straight to
  **[SETUP.md](telenotes-bot/SETUP.md)** to stand it up.
- **[telenotes-core/](telenotes-core/)** — shapes and pure text helpers
  shared by the bot and the TUI.
- **[telenotes-tui/](telenotes-tui/)** — a terminal client for browsing,
  searching, and capturing from a laptop.
- **[web/](web/)** — the dashboard, published to GitHub Pages: browse,
  search, add, edit, delete, and view stats.
- **[.github/workflows/](.github/workflows/)** — CI for the Rust workspace,
  the Supabase keepalive ping, and the Pages deploy.

Keep this repo **private**. The Pages *site* is public regardless — that's
deliberate, and the reason the dashboard ships nothing but the Supabase anon
key and leans on Row Level Security. See SETUP.md, step 7.

The captured content does not live here. Notes go to a separate private
"vault" repo as markdown, and files go to Cloudflare R2. Setting that up is
steps 2 and 4.

---

Extracted from [`Jayinaksha/reflect`](https://github.com/Jayinaksha/reflect)
(commit `c4cfe14`), where it had been sharing a repo with unrelated UAV/HRLLC
research.
