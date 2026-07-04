# ADR 0003 — Links are implemented natively (replacing Linkwarden)

Date: 2026-07-04 · Status: accepted

## Context

Links could be provided by keeping Linkwarden running and writing a client for its
REST API, or by implementing link management inside chaos-server.

## Decision

Native implementation in chaos-server: SQLite (sqlx) for links/collections/tags,
single-file page snapshots via the `monolith` crate for archiving, one-shot importer
from Linkwarden for migration.

## Consequences

- One fewer Node service on the host once migration completes.
- The data model (`chaos-domain::links`) is ours: hierarchical collections, tags,
  explicit `ArchiveState` lifecycle.
- We take on archiving robustness (timeouts, size limits, retries) — kept manageable
  by treating it as a background queue with per-link status rather than a
  synchronous operation.
