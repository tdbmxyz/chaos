# chaos — session handoff

## Current delivery model

Chaos is browser-only on phone and desktop: a responsive Leptos 0.8 CSR UI
built with trunk and served by Axum. Native desktop/Android shells, packaging,
platform bridges, and app-specific OIDC/PKCE/JWT authentication were removed.
Historical ADRs and `docs/superpowers/` plans describe the former design; do
not restore native build prerequisites from them.

See [ARCHITECTURE.md](ARCHITECTURE.md) for crate boundaries,
[deployment.md](deployment.md) for deployment and authentication, and
[ROADMAP.md](ROADMAP.md) for remaining work. The manga reader is a separate
project, [yomu](../../yomu).

## Features

- Dashboard: monitored services, proxied/cached icons, bookmark groups, search,
  configurable widget columns, systemd controls, RSS, releases, news, and stats.
- Links: SQLite, hierarchical collections, tags, page metadata, pagination,
  favicons, full-text search, and background `monolith` archiving. Import with
  `chaos-server import-linkwarden <export.json> [owner-username]`.
- News: HN/Lobsters tabs with cumulative 24h/48h/week rankings, a comment-tree
  reader, viewed-state syncing, and authenticated engagement analytics. The
  server sanitizes comment HTML; direct HN responses render as escaped text.
- Calendars: local event CRUD and cached ICS subscriptions with RRULE expansion,
  merged date ranges, all-day events, descriptions, and manual feed refresh.
- Home: Home Assistant temperature history, sensors, and light controls.
- Weather: direct Open-Meteo fetches, comparison charts, and device-local units
  and places. ECharts loads on demand.
- Preferences: device-local themes, weather settings, and optional API override.
  Desktop uses a sidebar; phones use a bottom tab bar and More page.
- Offline: health-probe connectivity, read-only cached data, disabled mutations,
  retry badge, and browser-online retry. HN can fetch Algolia directly;
  Lobsters uses cached server responses because its API does not allow CORS.
  Analytics outboxes flush on reconnect.
- Operations: ntfy alerts/reminders, scheduled SQLite backups, NixOS service
  module, and precompressed static assets with fingerprint-aware caching.

## Authentication

Local users are created with `chaos-server add-user <name>`; passwords use
Argon2id and sessions are opaque tokens, hashed at rest. Same-origin browsers
use the HttpOnly cookie. Explicit cross-origin API clients can use the session
bearer token.

Authentik browser SSO uses trusted forward-auth headers, guarded by a shared
secret. The proxy must strip client-supplied secret headers and stamp its own.
The authenticated username maps onto a chaos user; display names sync on rename.
API routes require authentication except health, login, and icon images; a
route-coverage test protects the allowlist. `/etc/nixos` now removes the native
OIDC/legacy-app settings, Chaos CORS and forward-auth bypass routes, and Athena
native package/module. The Authentik blueprint marks the retired Chaos OAuth
application/provider absent while retaining shared browser SSO. These host
changes require a NixOS rebuild and blueprint reconciliation to take effect.

## Development and conventions

- `nix develop`; `just server` (:4600) and `just web` (:8080, proxies `/api`).
- `just check`: formatting, workspace clippy, and wasm compilation.
- `just test`: workspace nextest tests.
- Packages: `.#chaos-server`, `.#chaos-web`, `.#chaos-web-static`.
- `chaos-domain` defines the wire contract; `chaos-client` handles HTTP;
  `chaos-ui` receives `AppConfig` through context. Server owns persistent writes.
- DB UUIDs are hyphenated text (v7), timestamps RFC3339. Migrations are embedded;
  row/domain mappings live in `db.rs`; database tests use in-memory SQLite.
- Background workers use `tokio::sync::Notify` with a safety poll.
- New flake files must be Git-tracked for Nix to see them. If wasm-bindgen
  changes, refresh both pinned CLI hashes in `flake.nix`.
- Production services may be running on this machine; use separate ports and
  temporary databases for development. Do not alter host deployment implicitly.
