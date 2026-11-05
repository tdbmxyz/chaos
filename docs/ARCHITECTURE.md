# Architecture

## Goal

A browser dashboard for local services, link management, and a household
calendar. Desktop and mobile use the same responsive web UI; there are no
native desktop or phone applications.

## Crate boundaries

```text
chaos-domain  ←  chaos-server
     ↑              │
chaos-client        │ serves /api/v1 + static dist
     ↑              │
 chaos-ui           │
     ↑              │
 chaos-web  ←──── browser
```

- **`chaos-domain`** defines wire types and the API contract. No I/O or
  framework dependencies; compiles for native Rust and wasm.
- **`chaos-client`** is the typed HTTP API client, using reqwest. It also
  implements direct browser fetches for Open-Meteo and Hacker News.
- **`chaos-server`** is the Axum backend, with SQLite via sqlx, background
  service monitoring, link archiving, and cached widget providers.
- **`chaos-ui`** contains Leptos CSR pages and components. `AppConfig` provides
  the API base and optional cross-origin session-token persistence.
- **`chaos-web`** is the trunk entrypoint, HTML, CSS, and browser assets.

The server is the only persistent-data writer. Device preferences, cached
responses, and offline analytics outboxes live in browser localStorage.

## Backend

- Versioned API under `/api/v1`, with shared domain DTOs.
- Configuration uses figment: defaults ← TOML ← `CHAOS_*` environment.
- SQLite stores links, collections, calendars, sessions, and analytics.
  Migrations are embedded; tests use in-memory databases.
- Service monitoring and widget fetching run server-side with bounded requests
  and per-provider caching. Archiving invokes `monolith` in a separate process.
- Authentication uses local password sessions (HttpOnly cookie, or an opaque
  bearer session token for explicit cross-origin clients) and trusted
  reverse-proxy forward-auth headers. The proxy identity is accepted only
  with the configured shared secret. Native-app OIDC/PKCE and JWT verification
  have been removed; browser SSO continues through the proxy.
- API routes require authentication except health, login, and icon images;
  a route-coverage test guards this allowlist.

## Frontend

- Leptos CSR built by trunk into a static bundle; no SSR or native webview.
- API base: optional `window.CHAOS_API_BASE` → Settings localStorage override
  → page origin → local development fallback. Trunk proxies `/api` in dev.
- A bounded health probe sets connectivity. Cached responses support read-only
  offline use; the badge and browser `online` event can retry the probe.
- Weather fetches Open-Meteo directly. Offline HN fetches Algolia directly;
  Lobsters uses cached server responses because its API does not allow CORS.
- Comment HTML is sanitized on the server. Direct HN responses use separate
  cache keys and render only as escaped text; provenance stays with the data.
- Responsive navigation and layouts serve phone and desktop browser widths.
  Themes and weather preferences are device-local.
- Wasm uses `[profile.wasm-release]` and `wasm-opt -Oz`. Trunk computes SRI
  after optimization; post-build mutation would break integrity checks.
- ECharts loads on demand only for the Home and Weather pages.

## Deployment

- `packages.chaos-server`: backend binary.
- `packages.chaos-web`: static trunk dist.
- `packages.chaos-web-static`: dist plus build-time Brotli/gzip siblings.
- `nixosModules.chaos`: service configuration and deployment wiring; see
  [deployment.md](deployment.md).

Static serving uses precompressed assets, immutable caching for fingerprinted
filenames, and revalidation for entrypoints and other unversioned files.
Historical plans and ADRs retain the former native-app design for reference.
