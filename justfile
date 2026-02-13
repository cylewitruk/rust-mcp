set dotenv-load := false

default:
  @just --list

fmt:
  cargo fmt

check:
  cargo check

clippy:
  cargo clippy --all-targets -- -D warnings

test:
  cargo test

run:
  cargo run

up:
  docker compose up --build -d

down:
  docker compose down

logs:
  docker compose logs -f rust-mcp

ps:
  docker compose ps
