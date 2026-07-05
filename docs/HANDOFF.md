# chaos — session handoff (2026-07-05, evening)

Fresh-session primer. Everything below is committed on `main`; working tree
clean. Companion project: [yomu](../../yomu) (manga app, own repo + HANDOFF).

## What chaos is

Glance replacement (dashboard for local services) + Linkwarden replacement
(links with archiving), one Rust workspace: Leptos 0.8 CSR (trunk) frontend
served by an Axum server, Tauri v2 scaffold for later desktop use. All
decisions in `docs/adr/`, phases in `docs/ROADMAP.md`.

## State: what works today (all verified end-to-end)

- **Dashboard**: service monitor (up/degraded/down + latency, 30s
  auto-refresh), icons proxied+cached server-side (`di:`/`si:`/`sh:` like
  glance), bookmarks groups from config, search bar (`search_url` template),
  manual refresh button.
- **Layout & widgets**: `[[columns]]` in config place widgets explicitly
  (sizes `full`/`small`; no columns ⇒ legacy single-column synthesized).
  Data widgets — weather (Open-Meteo, geocoded), feed (RSS/Atom, covers
  HN/lobsters), releases (GitHub `releases.atom`), server_stats (sysinfo,
  optional `mounts` filter) — are served per instance id from
  `GET /api/v1/widgets/{id}`, cached server-side with per-kind TTLs
  (10min/5min/30min/10s), stale payload on upstream failure. See
  `chaos-server/src/widgets/` and chaos.example.toml.
- **Calendar**: static month widget, fully client-side (Monday-first,
  prev/today/next).
- **Systemd manager**: `systemd` widget lists configured units with state
  dots and start/stop/restart buttons (`controllable = false` for
  status-only rows). Control via `POST /api/v1/widgets/{id}/systemd`;
  server enforces the config allowlist (422 otherwise) and shells out to
  systemctl (5s status TTL). Deployment needs
  `services.chaos.systemdControl` (static chaos user + scoped polkit
  rule) — recipe in docs/deployment.md.
- **Links**: SQLite (sqlx, WAL, FTS5), hierarchical collections
  (cycle-guarded), tags (case-insensitive, auto-GC), quick-add with page
  metadata fetch (og:/title, 6s/2MB bounded), edit dialogs, full-replacement
  PUT semantics.
- **Archiving**: background worker shells out to `monolith -q -j -I -k`;
  snapshots at `archives/<link-id>.html`, served CSP-sandboxed at
  `GET /api/v1/links/{id}/archive`; failures carry reasons; FTS5 (porter)
  over extracted page text — searchbox matches archived content.
- **Import**: `chaos-server import-linkwarden <export.json>` (collections
  incl. nesting, links, tags).
- **Nix**: `nix build .#chaos-server` / `.#chaos-web` both green (trunk runs
  in the sandbox with the lock-pinned wasm-bindgen-cli). NixOS module
  `services.chaos` eval-tested; deployment recipe in `docs/deployment.md`.

## Conventions (also apply to yomu)

- `chaos-domain` = wire contract, wasm-safe, no I/O. All HTTP via
  `chaos-client`. Platform config injected into `chaos-ui` via `AppConfig`
  context. Server is the only writer; clients stateless.
- Background work = single worker + `tokio::sync::Notify` + safety poll.
- DB: UUIDs as hyphenated TEXT (v7), RFC3339 timestamps, embedded
  migrations, row↔domain mapping only in `db.rs`, in-memory sqlite tests.
- Dev: `nix develop`; `just server` (:4600) + `just web` (:8080, proxies
  /api); `just check` before committing. Commits: `--no-gpg-sign`, grouped.
- Gotchas: git-add before nix commands (flake sees tracked files only);
  wasm-bindgen version change ⇒ refresh two hashes in flake.nix;
  cargo check of chaos-desktop needs `crates/chaos-web/dist/index.html`.

## Next steps (in rough value order)

1. **Deploy on zeus** (user action + assist): wire `nixosModules.chaos` into
   the system flake per docs/deployment.md, port the glance servicesList
   mapping, run alongside glance, then retire glance.
2. Phase 4 leftover: custom-api widget (user template) — low priority now
   that the systemd manager covers the real use case. Editable bookmarks
   still an open question.
3. Links polish: pagination UI (API supports limit/offset), FTS5 for
   titles/descriptions too, link icons/favicons, bulk actions.
4. Desktop (deferred until user has computer access): server URL picker,
   `cargo tauri icon` + bundle, flake package.
5. Auth story if ever exposed beyond LAN (axum middleware + token in
   chaos-client is the planned seam).
