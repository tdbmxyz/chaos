# Roadmap

Phases are ordered so each one ships something usable and none requires reworking
the previous one.

## Phase 0 — Foundations ✅ (this commit)

- [x] Cargo workspace, six crates, compile on native + wasm
- [x] Nix devshell (stable rust + trunk + wasm-bindgen-cli pinned by Cargo.lock + Tauri deps)
- [x] Axum server: `/api/v1/health`, `/api/v1/services`, static serving, config via figment
- [x] Service monitor task (glance `monitor` widget equivalent)
- [x] Leptos app shell: router, dashboard page rendering service tiles
- [x] Tauri v2 scaffold
- [x] Architecture docs + ADRs

## Phase 1 — Usable dashboard

- [x] Service icons (`di:`/`si:`/`sh:` like glance, proxied + cached server-side)
- [x] Bookmarks widget (static groups from config, like glance.nix today)
- [x] Search bar widget (configurable engine via `search_url`)
- [ ] Layout: columns/pages driven by config
- [x] Auto-refresh of service statuses (30s while the dashboard is open)
- [ ] Desktop: server URL setting (persisted) — waiting on the desktop focus

## Phase 2 — Links (Linkwarden core)

- [x] sqlx + SQLite in chaos-server, migrations, `Db` layer (in-memory tests)
- [x] CRUD API: links, collections (hierarchical, cycle-guarded), tags (auto-GC)
- [x] Metadata fetch on save (title, description; favicon later with icons)
- [x] Links UI: list, collection sidebar, tag filters, search
- [x] Quick-add (paste URL) and delete
- [x] Edit link / edit collection dialogs (incl. collection delete with confirm)

## Phase 3 — Archiving & import

- [x] Archive queue (background task) shelling out to `monolith` (single-file,
      JS-stripped, isolated snapshots; auto on save + manual re-archive)
- [x] Serve archived copies (`GET /api/v1/links/{id}/archive`, CSP-sandboxed)
- [x] Importer from Linkwarden export file (`chaos-server import-linkwarden <file>`)
- [x] Full-text search over archived content (SQLite FTS5, porter stemming)

## Phase 4 — More widgets

- [ ] Weather (Open-Meteo), calendar
- [ ] RSS/Hacker News/Lobsters feeds
- [ ] GitHub releases watcher
- [ ] Server stats (local host metrics)
- [ ] Custom API widget (user-defined template like glance's custom-api)

## Phase 5 — Deployment

- [x] Flake packages: `chaos-server`, `chaos-web` (trunk dist built in nix)
- [x] NixOS module `services.chaos` (freeform TOML settings, monolith on PATH,
      DynamicUser + /var/lib/chaos state) — see docs/deployment.md
- [ ] Replace glance in the system flake (host-side change; recipe in
      docs/deployment.md)
- [ ] Desktop bundle with icons (`cargo tauri icon`, `bundle.active = true`)

## Deferred / explicitly out of scope

- **Auth** — LAN-only posture for now; revisit if exposed beyond the LAN
  (axum middleware + token in chaos-client is the planned seam).
- **Manga/webtoon reader** — separate application, see [manga-app.md](manga-app.md).
- **Mobile** — Tauri v2 supports Android/iOS; the chaos-ui/shell split keeps the
  door open, but the manga app is the one that really needs mobile.
