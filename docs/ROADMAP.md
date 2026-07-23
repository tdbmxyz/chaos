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
- [x] RSS/Atom feeds (`feed` widget)
- [x] Native Hacker News + Lobsters widgets (points, comment counts,
      source label links to the discussion — RSS carries none of that;
      upstreams since the tabs work: HN via the Algolia archive API,
      lobsters via `newest.json` pagination)
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
- [x] Design decision: sidebar rail layout adopted as the base (bottom
      tabs on phone) after comparing columns/sidebar/hub/bento; palettes
      moved to a `/settings` page — Campbell default, GitHub Dark,
      Midnight, Daylight, Glass, Terminal

## Phase 8 — Candidates (in rough value order)

- [ ] Deploy on zeus, retire glance (recipe ready in docs/deployment.md)
- [ ] authentik (OIDC) sign-in once deployed (seam in ADR 0004)
- [x] Notifications: service down/recovered alerts (flap-debounced) +
      calendar reminders via ntfy (`[notifications]` in chaos.toml; web
      push not needed — ntfy app covers phones)
- [ ] Dashboard editing in-app (add/move/remove widgets, persisted
      server-side) instead of TOML-only layout
- [x] Quick-add from phone: Android share intent → `/?share=` →
      `/links?add=` auto quick-add (PWA share-target can reuse the seam)
- [x] Calendar polish: event descriptions in the day panel, feed refresh
      button — week/agenda view and per-calendar colors still open
- [x] Links polish: pagination UI (50/page), site favicons (`fav:` icon
      kind via DuckDuckGo, server-cached) — bulk actions still open
- [x] Server stats: ZFS-aware disks (statvfs per dataset lies about
      totals; datasets aggregate into per-pool rows, filtered datasets show
      own usage vs pool capacity, multi-mounts deduped) + 1h CPU/memory
      sparklines (30s sampler, only spawned when the widget exists)
- [ ] Todo/groceries widget (shared household lists, pairs with calendar)
- [x] Global quick-search across services, links and events (Ctrl-K):
      `GET /api/v1/search` + overlay (debounced, grouped, arrow-key nav)
- [x] Scheduled SQLite backup/export: `[backup]` config, VACUUM INTO
      snapshots with retention pruning
- [x] Offline core: connectivity decided by the health probe alone (no
      `navigator.onLine`), cache-first reads over localStorage for
      dashboard/services/widgets/links/calendar/home so last-known-good
      data keeps rendering, read-only offline semantics (mutations and
      service controls disabled), offline badge with manual retry plus
      the browser `online` event, 8s data / 3s health request deadlines
      so an unreachable server fails fast — ported from yomu's offline
      core
- [x] Direct-fetch weather: every client (web, desktop, Android) geocodes
      and fetches Open-Meteo itself instead of through the server —
      `GET /api/v1/weather` removed, per-place localStorage cache (600s TTL,
      serves stale on failure), so weather keeps working even offline
- [x] Direct-fetch HN + lobsters when the server is unreachable: HN via
      the CORS-open Algolia API on every client, lobsters via the Tauri
      shells' `tauri-plugin-http` (lobste.rs sends no CORS headers, so the
      plain web build serves lobsters from cache only while offline); a
      direct fetch overwrites the widget cache so later offline views see
      fresh leftovers
- [x] Hacker News and lobsters feeds ordered by upvotes (score descending,
      scoreless items last) instead of the upstream endpoint's own ranking
- [x] HN/lobsters time-window tabs: Last 24h / 48h / Week — each the
      top-by-upvotes of its whole trailing window (cumulative, so 48h/Week
      include today's big stories mixed in by score), deduped and capped.
      HN via the Algolia archive API (one weekly `tags=story`, `points>=50`
      query), lobsters via `newest/page/{N}.json` pagination (up to 10
      pages, deduped by id); offline, tabs keep working from the cached
      payload
- [x] Weather charts on a viewer-local time axis: every chart aligns
      places by real instant (fixes cross-timezone offset), alternating
      day bands + weekday/date tooltips, and the combined view is the
      persisted default, rendered above the per-place rows — still fully
      client-side (direct Open-Meteo)
- [x] Dedicated News page (`/news`): HN and lobste.rs sub-tabs with the
      24h/48h/Week windows, served from `GET /api/v1/posts/{source}`
      (offline direct fallback). Rows carry a source favicon (existing
      `fav:` icon proxy) that links straight to the article; the title
      opens the in-app reader. The phone dashboard drops the two posts
      widgets in favor of this page (desktop keeps them as glances); News
      takes Home's slot in the phone tab bar, Home moves to the More page
- [x] In-app comment reader (`/news/:source/:id`): story header plus a
      collapsible comment tree (short-press a comment to fold its own
      subtree, `[+N]` badge). Threads come from
      `GET /api/v1/posts/{source}/{id}/comments` — HN via the Algolia item
      API, lobste.rs via `/s/{id}.json` (flat depth list rebuilt into a
      tree) — with comment HTML sanitized server-side (ammonia allowlist).
      Offline falls back to a direct fetch rendered as plain text; the
      sanitized-vs-text decision travels with the payload so `inner_html`
      only ever renders server-sanitized HTML
- [x] Logarithmic score heat scale: `ln(1+score)/ln(1+p99)` instead of
      linear, so clustered mid-range scores stop collapsing into
      indistinguishable yellows
- [x] Per-user news viewed-state + engagement analytics (authed web +
      Android): rows on `/news` show seen (viewport → dimmed title) /
      opened-comments (dimmer) / opened-article (`✓` between domain and
      favicon, title kept bright) states, synced cross-device via three
      tables (`post_views` per-user first-timestamps, `posts` ingestion
      `first_seen_at`, `analytics_events` log) and auth-gated endpoints
      (`/posts/{source}/views`, `/posts/views`, `/analytics/events`). An
      offline outbox queues events and flushes on reconnect. Analytics
      events: `login`, `app_open` (≤1/5min/device), `search`, `reader_open`.
      Logged-off = plain rows, no tracking

- [x] Forward-auth (authentik) + app authentication + greeting: with
      `[forward_auth].secret` set, chaos trusts a reverse proxy that stamps
      `X-Chaos-Proxy-Secret` (anti-spoof guard) and forwards
      `X-authentik-username`/`-name`, resolving/auto-provisioning that user in
      the `AuthUser` extractor (session token still wins). The Tauri/Android app
      can authenticate to authentik with a per-device app-password
      (`Authorization: Basic`, Settings → Authentik) so it reaches the server
      behind SSO. A "Hello {name}" / "Hello stranger" greeting confirms it.
      Verified end-to-end (headless browser + curl-simulated proxy)

## Deferred / explicitly out of scope

- **Manga/webtoon reader** — separate application: [yomu](../../yomu).
- **CalDAV two-way sync** — only if writing to external calendars is ever
  needed; ICS read + local write covers the household case.
