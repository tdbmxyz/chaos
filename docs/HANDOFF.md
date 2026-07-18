# chaos — session handoff (2026-07-06)

Fresh-session primer. Everything below is committed on `main`; working tree
clean. Companion project: [yomu](../../yomu) (manga app, own repo + HANDOFF).

## What chaos is

Glance replacement (dashboard for local services) + Linkwarden replacement
(links with archiving) + household calendar, one Rust workspace: Leptos 0.8
CSR (trunk) frontend served by an Axum server, Tauri v2 scaffold for later
desktop use. All decisions in `docs/adr/`, phases in `docs/ROADMAP.md`.

## State: what works today (all verified end-to-end)

- **Dashboard**: service monitor (up/degraded/down + latency, 30s
  auto-refresh), icons proxied+cached server-side (`di:`/`si:`/`sh:` like
  glance), bookmarks groups from config, search bar (`search_url` template),
  manual refresh button.
- **Layout & widgets**: `[[columns]]` in config place widgets explicitly
  (sizes `full`/`small`; no columns ⇒ legacy single-column synthesized).
  Data widgets — weather (Open-Meteo, geocoded), feed (RSS/Atom),
  hacker_news (Algolia archive: top-by-upvotes per 24h/48h/week window,
  points + comment counts, source label → discussion), lobsters
  (newest.json paginated a week back, same windowed shape), releases (GitHub `releases.atom`), server_stats (sysinfo,
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
- **Import**: `chaos-server import-linkwarden <export.json> [owner-username]`
  (collections incl. nesting, links, tags; optional owner attributes every
  imported link via `links.created_by`, attribution only).
- **Auth**: users (`chaos-server add-user <name>`, argon2id) + sessions
  (opaque token, sha256 at rest, 90d; HttpOnly cookie for web, bearer for
  native). Logged-off works everywhere except calendars. authentik/OIDC is
  the planned next identity source — see docs/adr/0004-auth-and-calendar.md
  for the exact seam.
- **Calendar section**: `/calendar` tab (also via the dashboard calendar
  widget title). Per-user calendars: `local` (event CRUD in SQLite) and
  `ics` feed subscriptions (Google secret address / Proton share link),
  server-cached 10min with RRULE expansion (rrule crate). Merged range view
  `GET /api/v1/calendar/events?start&end`; broken feeds degrade to a
  warning. All-day events = symbolic UTC dates.
- **Mobile**: usable at phone widths (topbar wraps, single-column dashboard,
  calendar chips become dots, links stack) — verified via 390px screenshots.
- **Shells (Tauri v2, crates/chaos-desktop)**: same pattern as yomu-shell.
  The web bundle resolves its API base `window.CHAOS_API_BASE` →
  `localStorage["chaos-api-base"]` → page origin (`tauri.localhost` never
  trusted) → `127.0.0.1:4600`; a ServerGate health-check shows a connect
  form when unreachable. Cross-origin API ⇒ the session token is kept in
  localStorage and sent as bearer (`AppConfig.persist_token`); same-origin
  web keeps the HttpOnly cookie. Desktop: `just desktop [server]`,
  `CHAOS_SERVER` env / `~/.config/chaos/server`, NVIDIA DMABUF workaround,
  `just bundle` (deb) or `nix build .#chaos-desktop` (desktop entry +
  icons; AppImage dropped — linuxdeploy doesn't run on NixOS). Verified
  e2e: shell against a live server requested health/me/dashboard/icons/
  widgets. Android: `nix develop .#android` + `just apk`; gen/android
  committed with yomu's keystore-signing + cleartext-release edits;
  keystore + keystore.properties in `~/.config/chaos` (gitignored copy at
  gen/android/keystore.properties).
- **Server stats**: ZFS-aware — statvfs on a dataset reports
  `own usage + pool free` as "total", so datasets are aggregated into one
  row per pool (dedup by dataset name: zeus mounts each dataset twice,
  zfs mountpoint + /data bind). Datasets named in `mounts` show own usage
  against pool capacity; pool names work in `mounts` too. Snapshot usage
  is invisible to statvfs (few % under `zpool list`), accepted. Plus 1h
  CPU/mem sparklines: 30s sampler task (spawned only when a server_stats
  widget exists), history rides in the widget payload (`serde default`).
- **Share target (Android)**: share sheet → MainActivity loads
  `/?share=<text>` (only root resolves in the asset protocol) →
  ShareRedirect forwards to `/links?add=` → AddLinkForm extracts the first
  http(s) URL and saves immediately (no URL ⇒ prefill only). Web-tested
  e2e; needs one on-device check after the next APK build.
- **Calendar polish**: event descriptions shown in the day panel (also
  parsed from ICS DESCRIPTION), ↻ toolbar button → POST
  /api/v1/calendar/refresh invalidates the user's feed caches.
- **Links polish**: pagination (50/page, filters reset to page 1, uses
  LinkPage.total) and per-site favicons (`fav:<host>` icon kind →
  DuckDuckGo ip3, cached like other icons; empty upstream bodies 404
  instead of being cached).
- **Device preferences** (`/settings`, localStorage): theme, weather
  location (sent as `?location=` widget override, geocoded + cached per
  place server-side) and °C/°F (pure display conversion, wire stays
  metric).
- **Companion apps ("plugins") — removed in v1.4**: the old `[[apps]]`
  config → `GET /api/v1/apps` → sidebar iframe embed is gone. Replaced by
  `Bookmark.android_package`: any bookmark can carry an `android_package`,
  and the Android shell launches that app natively
  (`window.ChaosAndroid.openApp(pkg)`) when tapped, falling back to the
  plain URL everywhere (including Android when the app isn't installed). A
  stray `[[apps]]` section left in an old `chaos.toml` is silently ignored,
  not a startup error.
- **Design (decided 2026-07-06)**: the sidebar layout IS the app — fixed
  left nav rail (Settings + account pinned to the bottom) that becomes a
  bottom tab bar on phones; the alternative layouts (columns/hub/bento)
  were pruned after comparison. Palettes live on `/settings` (radio list
  with swatches, applied as `body[data-theme]`, persisted per device):
  Campbell (Windows Terminal scheme, the default), GitHub Dark, Midnight,
  Daylight, Glass, Terminal. Per-kind widget classes
  (`widget-services`, …) remain for styling hooks.
- **Nix**: `nix build .#chaos-server` / `.#chaos-web` / `.#chaos-desktop`
  green (trunk runs in the sandbox with the lock-pinned wasm-bindgen-cli).
  NixOS module `services.chaos` eval-tested; deployment recipe in
  `docs/deployment.md`.

## Conventions (also apply to yomu)

- `chaos-domain` = wire contract, wasm-safe, no I/O. All HTTP via
  `chaos-client`. Platform config injected into `chaos-ui` via `AppConfig`
  context. Server is the only writer; clients stateless.
- Background work = single worker + `tokio::sync::Notify` + safety poll.
- DB: UUIDs as hyphenated TEXT (v7), RFC3339 timestamps, embedded
  migrations, row↔domain mapping only in `db.rs`, in-memory sqlite tests.
- Dev: `nix develop`; `just server` (:4600) + `just web` (:8080, proxies
  /api); `just check` before committing. Commits: signed (GPG works
  again since 2026-07-06), grouped; remote git@github.com:tdbmxyz/chaos.
- Careful on this machine: it IS zeus — production `services.yomu`
  listens on 4700 (user yomu, untouchable); use another port for dev
  instances.
- Gotchas: git-add before nix commands (flake sees tracked files only);
  wasm-bindgen version change ⇒ refresh two hashes in flake.nix;
  cargo check of chaos-desktop needs `crates/chaos-web/dist/index.html`.

## Next steps (in rough value order — full list in ROADMAP phase 8)

1. **Theme decision** (user): pick from the five; make it the default and
   prune the rest (delete their CSS blocks + THEMES entries).
2. **Deploy on zeus** (user action + assist): wire `nixosModules.chaos` into
   the system flake per docs/deployment.md, port the glance servicesList
   mapping, run alongside glance, then retire glance. Install the phone
   APK / `nix build .#chaos-desktop` on the desktop.
3. authentik integration when the user deploys it: OIDC redirect flow +
   `sub`→user mapping; session layer unchanged (ADR 0004). Until then:
   `chaos-admin add-user tibo` / `add-user so` on zeus.
4. Notifications (service down, calendar reminders) via ntfy or web push.
5. Dashboard editing in-app; quick-add share target on Android.
6. Calendar/links polish (ROADMAP phase 8 has the itemized list).
7. If ever exposed beyond the LAN: put the remaining public routes behind
   `AuthUser` (one-line per route) and front with authentik.
