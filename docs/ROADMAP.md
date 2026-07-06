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
- [x] Layout: columns driven by config (`[[columns]]` of widgets; multi-page
      deferred until a real need shows up)
- [x] Auto-refresh of service statuses (30s while the dashboard is open)
- [x] Manual refresh button (re-runs every widget on the page)
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

Widgets are declared in config columns; data is fetched and cached
server-side, one instance id per widget (`GET /api/v1/widgets/{id}`).

- [x] Weather (Open-Meteo, geocoded location, 5-day forecast)
- [x] RSS/Hacker News/Lobsters feeds (one `feed` widget; HN/lobsters are
      plain RSS via hnrss.org / lobste.rs/rss)
- [x] GitHub releases watcher (`releases.atom`, no API token needed)
- [x] Server stats (host metrics via sysinfo; optional `mounts` filter)
- [x] Calendar (static month view, client-side; title links to the full
      calendar section)
- [x] Systemd services manager (native replacement for the glance
      custom-api + webservice workaround: unit states + start/stop/restart,
      config allowlist, polkit rule via `services.chaos.systemdControl`)
- ~~Custom API widget~~ — dropped: needed custom widgets become native
      instead (like the systemd manager)

## Phase 6 — Users & calendar section ✅

- [x] Users + sessions (argon2id passwords, opaque tokens sha256-hashed,
      cookie for web / bearer for native, `chaos-server add-user` CLI)
- [x] Logged-off as a first-class state (dashboard/links stay public;
      calendars per-user) — see docs/adr/0004-auth-and-calendar.md
- [x] Calendar section (`/calendar`, also via the widget title): month view,
      day panel, event create/edit/delete on local calendars
- [x] ICS feed subscriptions (Google secret address, Proton share link),
      server-cached, RRULE expansion
- [x] Mobile/vertical layout pass (topbar, dashboard, links, calendar)
- [ ] authentik (OIDC) as external identity provider — session layer is
      ready, needs the redirect flow + user mapping when authentik is up
- [ ] CalDAV two-way sync if writing to external calendars is ever needed

## Phase 5 — Deployment

- [x] Flake packages: `chaos-server`, `chaos-web` (trunk dist built in nix)
- [x] NixOS module `services.chaos` (freeform TOML settings, monolith on PATH,
      static chaos user + /var/lib/chaos state, `chaos-admin` host command)
      — see docs/deployment.md
- [ ] Replace glance in the system flake (host-side change; recipe in
      docs/deployment.md)

## Phase 7 — Shells & themes

Same shell pattern as yomu (yomu-shell): the web bundle runs inside Tauri,
the shell only injects `window.CHAOS_API_BASE`, the UI has a ServerGate
connect screen and keeps the session token in localStorage when the API is
cross-origin (bearer instead of the cookie).

- [x] API-base resolution seam (injected global → localStorage override →
      origin → fallback; `tauri.localhost` never trusted as API)
- [x] Desktop shell: `CHAOS_SERVER` env / `~/.config/chaos/server`, NVIDIA
      DMABUF workaround, deb + AppImage bundles (`just bundle`)
- [x] Android shell: `nix develop .#android` + `just apk` (signed release,
      keystore in ~/.config/chaos, gen/android committed)
- [x] Selectable themes (palette + nav layout, `data-theme` CSS): midnight,
      daylight, sidebar, glass, terminal — pick one, then prune the rest

## Phase 8 — Candidates (in rough value order)

- [ ] Deploy on zeus, retire glance (recipe ready in docs/deployment.md)
- [ ] authentik (OIDC) sign-in once deployed (seam in ADR 0004)
- [ ] Notifications: service down / calendar reminders via ntfy or web push
- [ ] Dashboard editing in-app (add/move/remove widgets, persisted
      server-side) instead of TOML-only layout
- [ ] Quick-add from phone: PWA share-target + Android share intent so
      links can be saved from any app
- [ ] Calendar polish: week/agenda view, event descriptions in the day
      panel, feed refresh button, per-calendar colors in the picker
- [ ] Links polish: pagination UI, favicons, bulk actions, FTS over
      titles/descriptions
- [ ] Server stats history (small ring buffer + sparklines)
- [ ] Todo/groceries widget (shared household lists, pairs with calendar)
- [ ] Global quick-search across services, links and events (Ctrl-K)
- [ ] Scheduled SQLite backup/export

## Deferred / explicitly out of scope

- **Manga/webtoon reader** — separate application: [yomu](../../yomu).
- **CalDAV two-way sync** — only if writing to external calendars is ever
  needed; ICS read + local write covers the household case.
