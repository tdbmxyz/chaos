# Architecture

## Goal

One application, two delivery modes, replacing glance as the home for local services:

1. **Web**: `chaos-server` serves the API and the built frontend on the LAN — the browser
   start page, exactly like glance today.
2. **Desktop app**: the same frontend wrapped in a Tauri window, talking to the same server
   over HTTP.

The split into crates enforces the boundaries that make this possible:

```
                 ┌────────────────┐
                 │  chaos-domain  │  types + API contract (no I/O)
                 └───┬───────┬────┘
                     │       │
            ┌────────┴──┐ ┌──┴───────────┐
            │chaos-client│ │ chaos-server │  axum + monitor + (soon) sqlite
            └────────┬──┘ └──┬───────────┘
                     │       │ serves /api/v1 + static dist
              ┌──────┴───┐   │
              │ chaos-ui │   │  leptos components (wasm)
              └───┬───┬──┘   │
                  │   │      │
        ┌─────────┴┐ ┌┴──────┴─────┐
        │chaos-web │ │chaos-desktop│  trunk entry / tauri shell
        └──────────┘ └─────────────┘
```

## Rules that keep refactoring cheap

- **`chaos-domain` is the contract.** Everything crossing the wire is defined there once,
  shared by server and clients. It has no async runtime, no framework deps, and compiles
  for wasm. Breaking changes to it are treated as API version changes.
- **All HTTP goes through `chaos-client`.** UI code never touches `reqwest` directly.
  When auth lands, it lands in one place.
- **`chaos-ui` is platform-agnostic.** Platform specifics (API base URL today; settings
  storage tomorrow) are injected by the shells (`chaos-web`, `chaos-desktop`) via
  `AppConfig`/context. This is the same seam the future mobile shell would use.
- **The server is the only writer.** Clients are stateless; the desktop app is a thin
  window onto the server. (The offline story belongs to the manga app, not chaos.)

## Backend

- **Axum** with a versioned API under `/api/v1`. DTOs come from `chaos-domain`.
- **Service monitor**: background tokio task polling each configured service
  (`HealthState::Up` for any response < 500, `Degraded` on 5xx, `Down` on transport
  errors), results kept in memory. This replaces glance's `monitor` widget.
- **Configuration**: figment (defaults ← TOML ← `CHAOS_*` env). On NixOS, the future
  `services.chaos` module generates the TOML from `config.modules.server.servicesList`,
  replacing `inspirations/glance.nix`.
- **Links (Phase 2)**: SQLite via sqlx, migrations in-repo. Schema follows
  `chaos-domain::links` (links, collections with hierarchy, tags, archive state).
  Archiving uses the `monolith` crate to store single-file page snapshots on disk,
  processed by a background queue. One-time import from Linkwarden's API/export.

## Frontend

- **Leptos CSR** built by trunk. No SSR: the frontend must be a static bundle so the
  identical artifact runs in the browser (served by axum) and inside Tauri. This also
  keeps the server deployable without a wasm toolchain.
- API base resolution: same-origin in the browser (trunk proxies `/api` in dev);
  inside Tauri (origin `tauri://localhost`) it falls back to a configured server URL.

## Deployment target (Phase 5)

- Flake gains `packages.chaos-server` (with the trunk dist baked in via `static_dir`)
  and a NixOS module `services.chaos` replacing `services.glance` in the system config.
- The desktop app ships via `cargo tauri build` / a flake package for Linux.

## Known deferred decisions

- **Auth**: none for now (LAN-only, same posture as glance). The seam is axum
  middleware + `chaos-client`; tracked in ROADMAP.
- **Database**: sqlx+SQLite arrives with Phase 2; the monitor stays in-memory on purpose.
- **Widgets beyond monitor/bookmarks** (weather, feeds, releases, server stats): Phase 4,
  each as a server-side provider with caching, mirroring glance's model where the server
  fetches and the client renders.
