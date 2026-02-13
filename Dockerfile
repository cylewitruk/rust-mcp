FROM rust:1.93-bookworm AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations

RUN cargo build --release --locked

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tzdata ripgrep \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --home-dir /home/rust-mcp --shell /usr/sbin/nologin rust-mcp

WORKDIR /app
COPY --from=builder /app/target/release/rust-mcp /usr/local/bin/rust-mcp

USER rust-mcp
EXPOSE 43173

ENTRYPOINT ["/usr/local/bin/rust-mcp"]
