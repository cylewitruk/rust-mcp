set dotenv-load := false

default:
  @just --list

build:
    cargo --locked build --all-targets --release

fmt:
    cargo +nightly --locked fmt --all

lint:
    RUST_LOG=warn cargo --locked clippy --all-targets -- -D warnings
    cargo check --locked --all-targets
    cargo +nightly --locked fmt --all -- --check

fix:
    RUST_LOG=warn cargo --locked clippy --fix --all-targets --allow-dirty
    cargo +nightly --locked fmt --all

test:
  cargo --locked nextest run --no-fail-fast --all-targets

run:
  cargo --locked run -p rust-mcp

cbuild:
  docker compose build rust-mcp

up:
  docker compose up rust-mcp --build -d

down:
  docker compose down

logs:
  docker compose logs -f rust-mcp

ps:
  docker compose ps
