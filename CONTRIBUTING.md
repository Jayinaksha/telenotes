# Contributing to telenotes

Thanks for taking a look. This is a small, single-maintainer project, so the
process is intentionally light — but a few conventions keep the codebase
consistent.

## Before you start

For anything beyond a small fix (a new feature, a change to the storage or
auth model, a new dependency), please open an issue or a
[Discussion](../../discussions) first. It's a quick way to avoid spending
time on a PR that doesn't fit the project's direction.

For typos, docs fixes, or an obvious bug fix, just open a PR directly.

## Project layout

See the [root README](README.md) for the workspace layout and
[architecture.md](architecture.md) for how the pieces fit together. The
short version:

- `telenotes-bot/` — the Rust bot and its `/api/*` HTTP surface
- `telenotes-core/` — shared types and pure helpers
- `telenotes-tui/` — the terminal client
- `web/` — the dashboard (plain JS, no build step)

Each Rust crate's `SETUP.md`/`README.md` (where present) covers what it does
and how to stand it up locally against a real Supabase project.

## Making a change

1. Fork and branch from `main`.
2. Make your change. Keep it scoped — a bug fix shouldn't carry along an
   unrelated refactor, and a new feature shouldn't touch code it doesn't
   need to.
3. Run the checks below before opening a PR. CI runs the same commands and
   will block on any failure.
4. Open a PR against `main` with a short description of *why*, not just
   *what* — the diff already shows what changed.

### Rust (`telenotes-bot`, `telenotes-core`, `telenotes-tui`)

From the repo root, so all three workspace members are covered:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

`cargo clippy` is run with warnings denied — a clippy warning is a CI
failure, not a suggestion.

### Web dashboard (`web/`)

No build step, no bundler, no test suite — it's plain JS served as-is by
GitHub Pages. Check your change by serving the directory locally (e.g.
`python3 -m http.server` from `web/`) and exercising the feature in a
browser against your own Supabase/bot setup (see
[`telenotes-bot/SETUP.md`](telenotes-bot/SETUP.md)).

## Code style

- Comments explain *why*, not *what* — the code already says what it does.
  If a comment doesn't carry a non-obvious reason (a constraint, a subtle
  invariant, a workaround), it's usually better deleted.
- Keep changes minimal and scoped to the task. No speculative abstractions,
  no unrelated cleanup riding along in the same diff.
- `rustfmt` and `clippy -D warnings` are the enforced style — don't fight
  them, and don't hand-format around what they'd otherwise catch.

## Commit messages and PRs

No fixed format is enforced, but aim for a short summary line focused on
*why* the change was made, with detail in the body if it's not obvious from
the diff alone. Squash-merge is fine — the individual commits on your branch
don't need to be pristine.

## Reporting bugs

Open an [issue](../../issues) with steps to reproduce. If it's a security
issue, please don't open a public issue — see [SECURITY.md](SECURITY.md)
instead.

## Questions

Use [Discussions](../../discussions) for anything that isn't a concrete bug
report or a specific code change — setup help, design questions, ideas.
