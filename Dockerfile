ARG RUST_VERSION=1.93
FROM rust:${RUST_VERSION}-alpine AS build

ENV CARGO_TARGET_DIR=/app/target
ENV CARGO_INCREMENTAL=0
ENV RUSTFLAGS="-C strip=symbols"
ENV CARGO_HOME=/usr/local/cargo

RUN apk add --no-cache build-base git openssl-dev pkgconfig

WORKDIR /app

COPY Cargo.toml Cargo.lock README.md ./

# Dummy source for dependency caching
RUN mkdir -p src && echo 'fn main(){}' > src/main.rs

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo fetch --locked

# Copy real source
COPY ./src ./src
COPY ./migrations ./migrations

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked && \
    install -D /app/target/release/rust-mcp /out/rust-mcp

# Final runtime image
FROM alpine:3.23 AS runtime

RUN apk add --no-cache ca-certificates tzdata ripgrep postgresql18 postgresql18-contrib su-exec iptables

# Separate users: postgres owns the DB, rust-mcp runs the application
RUN addgroup -S rust-mcp && adduser -S -G rust-mcp -H -s /sbin/nologin rust-mcp && \
    mkdir -p /var/lib/postgresql/data /run/postgresql && \
    chown -R postgres:postgres /var/lib/postgresql /run/postgresql

WORKDIR /app

COPY --from=build /out/rust-mcp /usr/local/bin/rust-mcp
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh

# Entrypoint runs as root, drops to postgres for DB and rust-mcp for the app
EXPOSE 43173 9090
VOLUME /var/lib/postgresql/data

ENTRYPOINT ["docker-entrypoint.sh"]
