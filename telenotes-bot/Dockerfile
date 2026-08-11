# Pinned to a toolchain this project is known to build on. The original
# rust:1.83 no longer works: current releases of the dependency tree need a
# newer compiler than that.
FROM rust:1.94-slim-bookworm AS builder
WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Build dependencies against a stub main first, so editing src/ doesn't
# recompile the whole tree on every deploy. Cargo.lock is committed, so this
# layer is reproducible.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release \
    && rm -rf src

COPY src ./src
# Cargo skips rebuilding if mtime looks unchanged after the stub swap.
RUN touch src/main.rs && cargo build --release

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
