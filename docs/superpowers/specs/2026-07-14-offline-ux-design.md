# Offline UX Design — chaos clients

Date: 2026-07-14
Status: approved

## Goal

Make the chaos clients (web, desktop, Android) usable when the chaos server is
unreachable: stop probing the server, keep browsing last-known-good data,
keep HN/lobsters fresh where the platform allows, and fetch weather directly
from Open-Meteo so it works with no server at all. Modeled on yomu's offline
architecture as refined for yomu 1.10.0.

## Context

Today ALL client network access goes through `ChaosClient` to `/api/v1/*`;
there is no connectivity signal, no cached data, and no request deadlines.
An unreachable server means every page shows an error and polls keep firing.
Weather is Open-Meteo (keyless) proxied by the server; HN/lobsters are fetched
server-side. `lobste.rs` sends no CORS headers (browser fetch blocked);
HN Firebase and both Open-Meteo APIs send `Access-Control-Allow-Origin: *`.

## Decisions (user-approved)

1. **Weather: always direct.** The client geocodes and fetches forecasts from
   Open-Meteo itself on every build, online or offline. The server weather
   proxy (`/api/v1/weather`, `widgets/weather.rs`, hub wiring) is removed.
2. **Feeds: direct in Tauri, cache on web.** When offline, desktop/Android
   fetch HN + lobsters natively via `tauri-plugin-http`; the web build fetches
   HN directly (CORS ok) and serves lobsters from cache. Online, both keep
   using the server's widget cache.
3. **Links: read-only cache.** No offline write outbox.
4. **Posts ordering:** HN and lobsters items are sorted by score (upvotes)
   descending after fetching — still sourced from `topstories`/`hottest.json`,
   not the dedicated "best" routes. Applies to both the server fetcher and the
   new direct client fetcher.

## Architecture

### 1. Connectivity core (`chaos-ui/src/offline.rs`, ported from yomu)

- `enum Connectivity { Checking, Online, Offline }` in an app-wide
  `RwSignal<Connectivity>`, provided from `App`; accessor `use_connectivity()`.
- **The health probe alone decides Online** (never `navigator.onLine`, never a
  per-request success — yomu commit `e4e08c5` lesson: request successes must
  not promote, or cached answers cause oscillation). Probe runs from: the boot
  `ServerGate`, the offline-badge retry button, and once on the browser
  `online` event. No background re-probe timers.
- The **first failed data request downgrades** Online → Offline (inside the
  `cached` helper).
- Request deadlines in `chaos-client`: `DATA_TIMEOUT = 8s`,
  `HEALTH_TIMEOUT = 3s`, so an unreachable host fails fast.
- `ServerGate` learns "server seen before" (localStorage `chaos-servers-seen`,
  keyed by base URL): a known server that is unreachable boots straight into
  the cached UI with `Connectivity::Offline` and an offline badge, instead of
  the connect form. An unknown server still gets the connect form.
- `OfflineBadge` component shown whenever `conn != Online`; it is the retry
  button ("connecting…" / "offline — retry" / "still offline" flash).

### 2. Cache-first reads

- `offline::cached(conn, key, fetch) -> Result<(T, bool), E>` — the one path
  for page reads. When `conn != Online` and a cached copy exists, return it
  immediately with `stale = true`, no network. Otherwise fetch; on success
  overwrite the cache (`stale = false`); on failure fall back to cache if
  present, and downgrade connectivity.
- Storage: localStorage `chaos-cache:<key>`, last-known-good JSON, no TTL
  (overwritten on next successful fetch).
- Cached keys: `dashboard` (layout), `services`, `widget:<id>`,
  `links` (default query only — first page, no filter), `collections`,
  `tags`, `calendar:<year-month>` + `calendars`, `home:sensors`.
- `use_polled_resource` gains connectivity awareness: while Offline the
  interval tick does not refetch; on recovery to Online resources refetch
  (they track `conn`).
- **Services: polling pauses entirely while offline** (no 30s poll); cached
  tiles render with status dots forced to the unknown state (statuses are
  stale) and start/stop buttons hidden/disabled.
- **Links: read-only offline** — cached default list, collections, tags
  browsable; create/edit/delete/archive controls disabled with an offline
  hint. Searching/filtering offline shows "unavailable offline" rather than
  stale-wrong results.
- All mutations everywhere (calendar events/calendars, lights, systemd
  actions) are disabled offline with a hint.
- Quick-search (Ctrl-K) offline: show "Search is unavailable offline".

### 3. Weather direct to Open-Meteo (`chaos-client::open_meteo`)

- New module in `chaos-client` (reqwest is already dual-target wasm/native)
  reproducing the server's logic: geocoding via
  `geocoding-api.open-meteo.com/v1/search`, forecast via
  `api.open-meteo.com/v1/forecast` (same params: 16 forecast + 16 past days),
  WMO code → description, producing `chaos_domain::WeatherData`.
- Caches (localStorage, managed by chaos-ui):
  - `chaos-cache:geocode:<name>` → `{lat, lon, resolved_name}`, permanent.
  - `chaos-cache:weather:<place>` → `{fetched_at, data}`, TTL 600s while
    online; served stale offline. `now_index` is recomputed from the hourly
    timestamps on read, so a cached forecast still points at the current hour.
- The weather page rows and the dashboard Weather widget call this module
  directly; they no longer call `/api/v1/weather` or `widget_data` for
  weather.
- Server removal: `GET /api/v1/weather` route + handler,
  `widgets/weather.rs`, `WidgetHub::weather()`, weather arm in the hub's
  `data()`/TTL table, `ChaosClient::weather()`. `Widget::Weather { location }`
  stays in config/domain (the dashboard layout still declares it); the server
  returns layout only. Breaking API removal is acceptable: clients and server
  ship together.

### 4. HN + lobsters direct when offline (`chaos-client::posts`)

- New module building `Vec<FeedItem>` (existing domain type):
  - HN: `topstories.json`, take first ~15 ids, fetch items concurrently,
    map to FeedItem (title, url, score, comments, comments_url).
  - Lobsters: `hottest.json` → FeedItem.
  - Both sorted by `score` descending after fetching.
- Platform matrix when offline: Tauri shells fetch both via
  `tauri-plugin-http` (`window.__TAURI__.http.fetch`), scoped by capability to
  exactly `lobste.rs`, `hacker-news.firebaseio.com`,
  `api.open-meteo.com`, `geocoding-api.open-meteo.com`. Web fetches HN via
  plain wasm fetch and serves lobsters from cache.
- Online path unchanged: both widgets keep polling the server's cached
  `widget_data` (300s).
- Server `widgets/posts.rs` also sorts both feeds by score descending
  (decision 4), so online and offline ordering match.
- Generic RSS and GitHub-releases widgets stay server-only: cache-only when
  offline. Out of scope.

### 5. Non-goals

- No service worker, no offline write outbox, no IndexedDB.
- No icon caching beyond the existing 30-day browser HTTP cache; the
  letter-tile fallback already covers missing icons offline.
- No background reconnect polling; recovery is manual (badge) or the browser
  `online` event.

## Error handling

- `cached` never surfaces a transport error when a cached copy exists — the
  UI shows stale data plus the offline badge instead.
- With no cache and no network, pages show their existing error/empty states.
- Direct Open-Meteo / feed fetch failures while offline fall back to cache,
  then to the widget's error state; they never flip connectivity (only chaos
  server requests do).

## Testing

- Unit: cache round-trip + stale flag; connectivity downgrade rules (failure
  downgrades, success never promotes); `now_index` recompute from hourly
  timestamps; Open-Meteo geocode/forecast JSON mapping from fixture strings;
  HN/lobsters JSON mapping from fixtures; score-descending sort on both
  fetchers (server + client).
- Existing workspace suite stays green; `cargo clippy -D warnings`, fmt,
  wasm targets build.
- Manual pass: stop the server → dashboard/links/weather/calendar render
  cached; badge retry recovers; weather works with server down.
