# ADR-0001: Read-through Refresh Strategy with Adaptive TTL

- Status: Accepted
- Date: 2026-02-13
- Deciders: rust-mcp maintainers
- Related: `docs/agent-dependency-mcp-spec.md` (Draft v0.2)

## Context

The initial plan allowed broad refresh behavior, but the product is local-first and interaction-driven. A full scheduled crawl can:

- create unnecessary traffic to crates.io/docs/security feeds,
- increase storage churn for low-value transient crates,
- consume background resources without improving active user outcomes.

At the same time, stale data can hurt package-update workflows where users need current information.

## Decision

Adopt a **read-through + stale-while-revalidate** model with **adaptive per-crate TTL**.

### Core behavior

1. `crate.search` and `crate.intel` are interaction triggers.
2. If crate TTL is valid, serve local indexed data.
3. If TTL expired, perform a lightweight freshness probe inline.
4. If unchanged, update freshness metadata only.
5. If changed, perform minimal inline refresh required for the active request and enqueue deep refresh.
6. If requested version is missing, bypass TTL and perform targeted inline fetch, then enqueue deep backfill.

### Queue behavior

- Use durable `refresh_job` records.
- Deduplicate pending/running jobs by `(crate_name, scope)`.
- Prioritize interactive/missing-version jobs over background jobs.
- Apply bounded worker concurrency and retries.
- Schedule retries with delayed requeue (`requested_at`) and bounded backoff.
- Add jitter to retry delay to reduce synchronized retry spikes.

### TTL behavior

- TTL is computed per crate using recency/frequency/probe outcomes (and optional security pressure).
- Enforce floor and cap (e.g., min 1h, max 90d).
- Add jitter to avoid synchronized refresh spikes.

## Consequences

### Positive

- Keeps highly used crates fresher with less unnecessary network traffic.
- Preserves low-latency responses while still converging to fresh state.
- Aligns resource use with actual MCP interaction patterns.

### Trade-offs

- Adds complexity (freshness metadata, probe logic, queue/worker lifecycle).
- Requires careful telemetry to tune TTL and queue policies.
- Can still return stale data when probes fail; responses must surface freshness state.

## Alternatives considered

1. **Global scheduled refresh (all crates)**
   - Rejected: excessive traffic/work for cold crates; weak alignment with interaction-driven use.

2. **No background refresh (strict inline only)**
   - Rejected: increased tail latency and repeated expensive fetch paths for deep data.

3. **Manual refresh only**
   - Rejected: too much user burden and higher risk of stale data in normal flows.

## Guardrails and observability

- Rate-limit outbound probes/refreshes per source.
- Expose queue depth/running jobs/errors in `index.status`.
- Include refresh/freshness fields in tool responses when relevant.
- Track metrics for probe outcomes, queued jobs, refresh latency, and remote call volume.

## Implementation notes

- This ADR defines behavior; exact TTL formula and thresholds are configurable.
- Implemented in current codebase:
  - `crate.intel` performs interaction-time freshness checks and missing-version targeted backfill.
  - `crate.search` performs bounded interaction freshness checks on top-ranked results.
  - worker dequeues only due jobs (`requested_at <= NOW()`), ordered by priority then attempts.
  - worker transitions: `pending -> running -> finished|pending(retry)|failed`.
  - retry delay uses bounded exponential backoff with jitter.
  - `index.status` exposes queue states (`pending`, `delayed`, `retrying`, `running`, `failed`) plus retry/failure distributions.
- Future ADRs may refine:
  - adaptive TTL weighting,
  - source-specific freshness probes,
  - strict freshness modes for upgrade-critical workflows.
