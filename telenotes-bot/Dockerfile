# Pinned to a toolchain this project is known to build on. Bumped from
# 1.83, then 1.94: pulling telenotes-tui into the workspace unified the
# dependency resolution and pushed `takecell` (a transitive dependency) to a
# version requiring rustc 1.96+.
#
# Built from the repo root (see render.yaml's dockerfilePath), not from
# telenotes-bot/ -- the bot is a workspace member depending on
# ../telenotes-core via a path dependency, so the build context has to see
# the whole workspace.
FROM rust:1.97-slim-bookworm AS builder
WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Build dependencies against stub crates first, so editing src/ doesn't
# recompile the whole tree on every deploy. Cargo.lock is committed, so this
# layer is reproducible. `-p telenotes-bot` never touches telenotes-tui's
# dependency tree (ratatui, crossterm, ...) -- it just needs telenotes-tui's
# manifest and a buildable stub to exist, because cargo resolves the whole
# workspace manifest regardless of which package you ask it to build.
COPY Cargo.toml Cargo.lock ./
COPY telenotes-core/Cargo.toml telenotes-core/Cargo.toml
COPY telenotes-bot/Cargo.toml telenotes-bot/Cargo.toml
COPY telenotes-tui/Cargo.toml telenotes-tui/Cargo.toml
RUN mkdir -p telenotes-core/src telenotes-bot/src telenotes-tui/src \
    && echo '' > telenotes-core/src/lib.rs \
    && echo 'fn main() {}' > telenotes-bot/src/main.rs \
    && echo 'fn main() {}' > telenotes-tui/src/main.rs \
    && cargo build --release -p telenotes-bot \
    && rm -rf telenotes-core/src telenotes-bot/src

COPY telenotes-core/src telenotes-core/src
COPY telenotes-bot/src telenotes-bot/src
COPY telenotes-bot/certs telenotes-bot/certs
# Cargo skips rebuilding if mtimes look unchanged after the stub swap.
RUN touch telenotes-core/src/lib.rs telenotes-bot/src/main.rs \
    && cargo build --release -p telenotes-bot

FROM debian:bookworm-slim
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --uid 10001 telenotes

COPY --from=builder /app/target/release/telenotes-bot /usr/local/bin/telenotes-bot

USER telenotes
# Render injects $PORT; the app reads it at startup and defaults to 8080.
EXPOSE 8080
CMD ["telenotes-bot"]
