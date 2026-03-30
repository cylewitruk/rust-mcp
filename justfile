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
  cargo --locked llvm-cov nextest \
    --workspace \
    --lcov \
    --output-path ./target/lcov.info \
    --no-fail-fast \
    --all-targets \
    --features integration-tests \
    --exclude rust-mcp-stdio
  cargo --locked nextest run \
    -p rust-mcp-stdio \
    --no-fail-fast

test-live:
  RUST_MCP_LIVE_CARGO_REGISTRY_DIR=~/.cargo/registry \
    cargo --locked nextest run \
      -p rust-mcp \
      --features live-tests \
      --test live \
      --no-fail-fast \
      --test-threads 1

test-e2e:
  docker build -t rust-mcp:test-e2e --build-arg PROFILE=dev -f Dockerfile .
  RUST_MCP_TEST_IMAGE_TAG=test-e2e \
    cargo --locked nextest run \
      -p rust-mcp \
      --test e2e_http \
      --no-fail-fast \
      --features e2e-tests

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
