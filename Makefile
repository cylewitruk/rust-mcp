.PHONY: help fmt check clippy test run up down logs ps

help:
	@echo "Targets:"
	@echo "  make fmt      - format Rust code"
	@echo "  make check    - cargo check"
	@echo "  make clippy   - run clippy with warnings denied"
	@echo "  make test     - run tests"
	@echo "  make run      - run server locally"
	@echo "  make up       - start docker stack"
	@echo "  make down     - stop docker stack"
	@echo "  make logs     - tail rust-mcp service logs"
	@echo "  make ps       - show docker stack status"

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
