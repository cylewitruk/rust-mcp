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

RUN apk add --no-cache ca-certificates tzdata ripgrep

RUN addgroup -S rust-mcp && adduser -S -G rust-mcp -H -s /sbin/nologin rust-mcp

WORKDIR /app

COPY --from=build /out/rust-mcp /usr/local/bin/rust-mcp

USER rust-mcp
EXPOSE 43173

ENTRYPOINT ["/usr/local/bin/rust-mcp"]
