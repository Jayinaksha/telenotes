# Security Policy

telenotes handles real credentials and personal data — Telegram bot tokens,
Supabase service/session tokens, storage credentials, and whatever you've
captured through it. Please report vulnerabilities privately so users have
a chance to update before an issue is public.

## Reporting a vulnerability

Preferred: use GitHub's **[private vulnerability reporting](../../security/advisories/new)**
for this repo (Security tab → "Report a vulnerability"). It opens a private
discussion with the maintainer and, if confirmed, can track the fix through
to a published advisory.

If that's unavailable to you for some reason, open a
[Discussion](../../discussions) marked as sensitive as possible, or reach
out to the maintainer directly via their GitHub profile — just don't open a
public issue with exploit details.

Please include:

- What component is affected (`telenotes-bot`, `telenotes-tui`, `web/`, or
  the Postgres schema/RPCs in `telenotes-bot/migrations/`)
- Steps to reproduce, or a proof of concept
- What you'd expect to happen instead

## Scope

In scope:

- Authentication and authorization: Telegram webhook verification, Supabase
  Auth + Row Level Security, the capability-token flows (`issue_write_token`,
  `issue_download_token`) in `telenotes-bot`
- Anything that could let one user read, modify, or delete another user's
  notes, files, or account
- Credential or secret exposure (webhook secrets, storage credentials,
  service-role-equivalent access) via the bot, dashboard, or TUI
- Injection (SQL, command, etc.) anywhere in the Rust services

Out of scope:

- Denial of service against the free-tier hosting this project is built
  for (Render's free plan, GitHub Pages, Supabase's free tier) — those
  platforms' own limits apply and are a known trade-off, not a
  telenotes-specific vulnerability
- Issues that require access to someone's own Telegram account, Supabase
  login, or local machine to exploit
- Vulnerabilities in third-party dependencies without a demonstrated,
  telenotes-specific impact (report those upstream; `cargo audit` findings
  without a reachable exploit path are welcome as a regular issue, not a
  security report)

## Supported versions

This project doesn't maintain multiple release branches — only the latest
code on `main` is supported. If you're running an older deployment, please
update before reporting, if practical.

## Response

This is a single-maintainer, spare-time project, so response time isn't
guaranteed — but security reports get priority over everything else in the
backlog.
