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
  cargo --locked nextest run

run:
  cargo --locked run -p rust-mcp

compose-up:
  docker compose up --build -d

compose-down:
  docker compose down

compose-logs:
  docker compose logs -f rust-mcp

compose-ps:
  docker compose ps
