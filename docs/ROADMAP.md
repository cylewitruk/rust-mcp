# ROADMAP

Status: Active
Last updated: 2026-03-06

This file tracks only open work and future direction.

## P1: Rustdoc Intelligence Quality

Canonical path resolution determines the shortest public import path for each symbol/type (e.g. `tokio::io::AsyncRead` instead of `tokio::io::async_read::AsyncRead`). This affects `crate_import_path`, `crate_api`, `crate_type_info`, and any tool that surfaces import paths.

- [x] Resolve glob re-exports in canonical path computation.
  - `pub use submodule::*` re-exports now contribute shorter canonical paths. Previously `build_canonical_path_map` skipped `is_glob: true` items entirely.
- [ ] Hardcode well-known standard library path remappings.
  - External types from `std`/`core`/`alloc` sometimes appear with internal module paths (e.g. `std::collections::hash_map::HashMap` instead of `std::collections::HashMap`). A static lookup table for common std re-exports would fix these without loading external rustdoc JSON.
- [ ] Cross-crate canonical path resolution for non-std dependencies.
  - When a crate's API surfaces types from its dependencies (e.g. `serde::Serialize` in trait bounds), the path comes from `krate.external_crates` and may be an internal path of the external crate. Resolving this requires looking up the external crate's already-indexed canonical paths from the database. Adds complexity and a dependency-ordering constraint (external crate must be indexed first).

## P2: First-Touch Latency

- [ ] Evaluate whether first-touch latency needs further optimization.
  - On-demand indexing typically completes in 1-3s for most crates. The coordinator timeout ceiling is 45s, which could theoretically be hit for very large crates or slow docs.rs downloads, but this is uncommon in practice.
  - If latency becomes a problem for specific crates, options include: returning syn-only data immediately with a "rustdoc enrichment in progress" indicator, or adding `_meta.estimated_wait` to progress notifications based on stored indexing statistics.

## P1: GitHub Integration (No Auth Required)

GitHub REST API endpoints below are publicly accessible at 60 req/hour without authentication. This is sufficient for on-demand lookups but not bulk indexing.

- [x] Add GitHub repo metadata sync for indexed crates.
  - Parses `owner/repo` from `repository` URL, fetches `GET /repos/{owner}/{repo}` for: star count, fork count, open issue/PR count, `archived` flag, `pushed_at` (last commit), license from repo metadata.
  - Stored in `github_repo_metadata` table and refreshed periodically (staleness-gated, using security sync interval).
  - Surfaced in `crate_intel` as a `github` section (maintenance health signal).
- [x] Add GHSA advisory cross-referencing.
  - `GET /advisories?ecosystem=rust&package={name}` returns GitHub Security Advisories for a crate.
  - Integrated into the existing security sync pipeline alongside OSV and RustSec.
  - Advisory matches are surfaced in `dependency_audit` and `crate_intel` security sections via the shared `advisory_matches` table.
- [x] Rate limiting for GitHub API.
  - Added `GITHUB_BASE_URL`, `GITHUB_MIN_INTERVAL_MS`, `GITHUB_WINDOW_*` config (following existing crates.io/docs.rs/OSV pattern).
  - Added `OutboundSource::GitHub` to the two-tier rate limiter.

## P2: GitHub Integration (Unauthenticated Enrichment)

These endpoints work without authentication at the default rate limits (1 req/5s burst, 59/hr window). A future `GITHUB_TOKEN` option would raise limits to 5,000 req/hour for bulk indexing.

- [x] Contributor count.
  - `GET /repos/{owner}/{repo}/contributors?per_page=1&anon=true` — uses `Link` header pagination to extract total contributor count in a single request. Stored in `github_repo_metadata.contributor_count`, surfaced in `crate_intel` `github.contributors`.
- [x] GitHub release notes.
  - `GET /repos/{owner}/{repo}/releases?per_page=10` — fetches latest releases (excluding drafts). Stored in `github_releases` table, surfaced in `crate_api_diff` and `crate_migration_path` as `release_notes` for human-readable upgrade context.
- [x] Background bulk GitHub metadata sync.
  - Runs at startup and on `SECURITY_SYNC_INTERVAL_SECS` cadence (default 24h). Processes up to 50 crates per pass with staleness gating. Works at unauthenticated rate limits (~19 crates/hr at 3 requests each).
  - A future `GITHUB_TOKEN` option would raise limits to 5,000 req/hour for larger registries.
- [x] Commit liveness via git probe.
  - Shallow bare `git clone --depth=N` (configurable via `GIT_PROBE_CLONE_DEPTH`, default 500) extracts `last_commit_at`, `last_commit_message`, and `recent_commit_count` (commits in last 90 days) without consuming API rate limits.
  - Stored in `github_repo_metadata`, surfaced in `crate_intel` `github` section.
  - Configurable: `GIT_PROBE_ENABLED` (default true), `GIT_PROBE_CLONE_DEPTH` (default 500), `GIT_PROBE_TIMEOUT_SECS` (default 60).
- [ ] Community health metrics.
  - `GET /repos/{owner}/{repo}/community/profile` (authenticated) — community health percentage (has README, contributing guide, code of conduct, etc.).

## P2: Indexing and Operations

- [ ] Optional container-side rustdoc JSON generation workflow (nightly, bounded/isolated execution).
  - _**Background:** docs.rs does not have rustdoc-json built for all versions of all crates yet. Many crate versions published prior to May 2025 are not available for download._
  - Local rustdoc builds should be triggered automatically for discovered versions that lack docs.rs artifacts.
- [ ] Background refresh fairness/backpressure tuning for large local registries.
- [ ] More granular per-source/per-tool SLO metrics and alertable counters.
- [ ] CI split for faster feedback: lint, integration, and e2e lanes with artifact reuse.

## P2: Deployment Hardening

- [ ] Verify `MCP_HTTP_BIND` default (`127.0.0.1`) works correctly with Docker port mapping; document override to `0.0.0.0` for custom compose files.
- [ ] Add production troubleshooting guide (DB connection failures, outbound firewall issues, rustdoc 404s for old versions, first-run expectations).

## P2: Optional Advanced Intelligence

- [ ] Evaluate optional rust-analyzer-assisted enrichment for workspace-local, position-aware context where rustdoc/syn are insufficient.
- [ ] Consider feature-gated API surfacing tools once rustdoc JSON exposes structured feature-gate data (currently only raw `Attribute::Other(String)` strings — not actionable).

## Out of Scope (For Now)

- Multi-tenant remote deployment model.
- Non-Rust ecosystems.
- Full Cargo resolver parity as a hard guarantee (current behavior remains best-effort with confidence signaling).
