# Direct Fetch (Weather + Posts) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Weather fetched directly from Open-Meteo by every client (server proxy removed); HN + lobsters fetched directly when the server is unreachable (HN everywhere, lobsters via the Tauri HTTP plugin); both feeds ordered by upvotes.

**Architecture:** An `open_meteo` module in `chaos-client` (reqwest is dual-target) reproduces the server's geocode+forecast logic and returns `WeatherData` plus the location's UTC offset; chaos-ui caches per-place forecasts in localStorage with a 600s TTL and recomputes `now_index` on cached reads. A `posts` module in `chaos-client` fetches HN and parses lobsters JSON; the lobsters bytes come through `window.__TAURI__.http.fetch` in the shells (lobste.rs sends no CORS headers). Depends on the offline-core plan (Connectivity, `cached()`, `use_polled_resource_with`) being merged first. Spec: `docs/superpowers/specs/2026-07-14-offline-ux-design.md`.

**Tech Stack:** Rust, reqwest dual-target, Leptos 0.8, tauri-plugin-http (Tauri v2), js-sys/wasm-bindgen-futures interop, localStorage.

**Conventions for every task:** Commit with trailers
```
Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
```
and **`git -c commit.gpgsign=false commit …`** (signing YubiKey unavailable). Never push. `cargo fmt --all`; `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` green at every commit; wasm target must build for touched UI/client crates. `git add` before any `nix` command (flake builds only see tracked files). IDE diagnostics can be stale — trust cargo output.

---

### Task 1: `open_meteo` module in chaos-client

**Files:**
- Create: `crates/chaos-client/src/open_meteo.rs`
- Modify: `crates/chaos-client/src/lib.rs` (`pub mod open_meteo;`)
- Modify: `crates/chaos-client/Cargo.toml` (needs `chrono` with serde — check; chaos-domain already pulls it. Add `serde`, `serde_json` (dev), `futures` if missing.)

This is a port of `crates/chaos-server/src/widgets/weather.rs` (read it in full first — the plan below names the pieces; copy the originals verbatim where indicated so behavior is identical). Differences from the server version: no `StaleCache` (chaos-ui caches in localStorage), returns `WeatherData` + UTC offset instead of `WidgetData`, request errors are plain `String`, and requests carry an 8s deadline.

- [ ] **Step 1: Write failing tests first**

Create the module skeleton with the test module. Port ALL existing tests from `widgets/weather.rs` (`now_index_*`, `forecast_tolerates_null_tail_entries`) plus these new ones:

```rust
#[test]
fn recomputed_now_index_tracks_the_location_local_clock() {
    let hourly = vec![hour(9, 13), hour(9, 14), hour(9, 15)];
    // Location is UTC+2; at 12:30 UTC the local hour is 14:00.
    let utc = chrono::NaiveDate::from_ymd_opt(2026, 7, 9)
        .unwrap()
        .and_hms_opt(12, 30, 0)
        .unwrap()
        .and_utc();
    assert_eq!(recompute_now_index(&hourly, utc, 2 * 3600), 1);
}

#[test]
fn forecast_carries_the_utc_offset() {
    // Same fixture as forecast_tolerates_null_tail_entries plus
    // "utc_offset_seconds": 7200 at the top level.
    // assert parsed.utc_offset_seconds == 7200
}

#[test]
fn geocode_response_prefers_the_country_filtered_hit() {
    let raw = r#"{"results":[
        {"name":"Paris","latitude":33.66,"longitude":-95.55,"country":"United States","country_code":"US"},
        {"name":"Paris","latitude":48.85,"longitude":2.35,"country":"France","country_code":"FR"}
    ]}"#;
    let place = pick_place("Paris, FR", serde_json::from_str(raw).unwrap()).unwrap();
    assert_eq!(place.name, "Paris, FR");
    assert!((place.latitude - 48.85).abs() < 0.01);
}
```

Run: `cargo test -p chaos-client open_meteo` — Expected: FAIL (functions not defined).

- [ ] **Step 2: Implement**

Public surface:

```rust
//! Direct Open-Meteo access (no API key): geocoding + forecast, shared by
//! every client build. The server no longer proxies weather — this is THE
//! weather path, at home and away.

use std::time::Duration;

use chaos_domain::{DailyForecast, HourlyForecast, WeatherData};
use serde::{Deserialize, Serialize};

const TIMEOUT: Duration = Duration::from_secs(8);

/// A geocoded place — serializable because chaos-ui caches it per name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Place {
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
}

/// A fetched forecast plus what's needed to keep a cached copy honest:
/// `utc_offset_seconds` lets a reader recompute `now_index` for the
/// location-local current hour long after the fetch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaceForecast {
    pub data: WeatherData,
    pub utc_offset_seconds: i32,
}

pub async fn geocode(http: &reqwest::Client, location: &str) -> Result<Place, String>;
pub async fn forecast(http: &reqwest::Client, place: &Place) -> Result<PlaceForecast, String>;

/// Where "now" sits in a (possibly cached) hourly series: convert UTC now
/// to the location's local clock, truncate to the hour, find the first
/// entry at or after it.
pub fn recompute_now_index(
    hourly: &[HourlyForecast],
    now_utc: chrono::DateTime<chrono::Utc>,
    utc_offset_seconds: i32,
) -> usize;
```

Implementation notes:
- Copy verbatim from the server module: the forecast URL (same params incl. `forecast_days=16&past_days=16`), `build_series`, `now_index` (keep it private; `recompute_now_index` truncates minutes/seconds then delegates to it), `describe`, `parse_local_time`/`local_time`/`local_times` deserializers, and the `Forecast`/`CurrentWeather`/`HourlySeries`/`DailySeries`/`GeocodeResponse`/`GeocodeHit` structs. Add `utc_offset_seconds: i32` to `Forecast` (Open-Meteo returns it top-level whenever `timezone=auto` — it does; `#[serde(default)]` for safety) and update the null-tail fixture accordingly.
- `forecast()` assembles `WeatherData` exactly like the server's `fetch()` did (today-filter on daily, `now_index` from `current.time`), then wraps it in `PlaceForecast`.
- Extract the country-filter selection from the server's `resolve()` into a pure `fn pick_place(location: &str, response: GeocodeResponse) -> Result<Place, String>` so it's testable; `geocode()` = URL build + GET + `pick_place`. Copy the `rsplit_once(',')` disambiguation and the `"{name}, {CC}"` naming as-is.
- HTTP: a tiny private helper since chaos-server's `http_util` isn't available here:

```rust
async fn get_json<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    url: &str,
) -> Result<T, String> {
    let mut request = http
        .get(url)
        .build()
        .map_err(|e| e.to_string())?;
    *request.timeout_mut() = Some(TIMEOUT);
    let response = http.execute(request).await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status().as_u16()));
    }
    response.json().await.map_err(|e| e.to_string())
}
```

- `urlencoded` helper: chaos-client already depends on `url`; copy the server's one-liner.

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p chaos-client && cargo build -p chaos-client --target wasm32-unknown-unknown`
Expected: all PASS (ported + new), wasm builds.

- [ ] **Step 4: Commit**

```bash
git add crates/chaos-client && git -c commit.gpgsign=false commit -m "feat(client): direct Open-Meteo geocoding + forecast module"
```

---

### Task 2: chaos-ui weather goes direct (localStorage caches)

**Files:**
- Create: `crates/chaos-ui/src/weather_fetch.rs`
- Modify: `crates/chaos-ui/src/lib.rs` (`mod weather_fetch;`)
- Modify: `crates/chaos-ui/src/pages/weather.rs` (WeatherRow L344-361)
- Modify: `crates/chaos-ui/src/pages/dashboard.rs` (DataWidget weather arm)

- [ ] **Step 1: The cache-and-fetch layer, pure parts first (failing tests)**

`weather_fetch.rs` with tests for the pure staleness decision:

```rust
#[test]
fn cache_is_fresh_within_ttl_and_stale_after() {
    assert!(is_fresh(1_000_000.0, 1_000_000.0 + 599_000.0));
    assert!(!is_fresh(1_000_000.0, 1_000_000.0 + 601_000.0));
}
```

Run: `cargo test -p chaos-ui weather_fetch` — Expected: FAIL.

- [ ] **Step 2: Implement**

```rust
//! Client-side weather: direct Open-Meteo with per-place localStorage
//! caches. Weather never touches the chaos server, so it neither depends
//! on nor affects the Connectivity signal — its own failure handling is
//! TTL + serve-stale.

use chaos_client::open_meteo::{self, Place, PlaceForecast};
use chaos_domain::WeatherData;
use serde::{Deserialize, Serialize};

/// Forecasts refresh at most every 10 minutes (matches the TTL the server
/// cache used).
const TTL_MS: f64 = 600.0 * 1000.0;

#[derive(Serialize, Deserialize)]
struct CachedForecast {
    /// js Date.now() epoch millis at fetch time.
    fetched_at_ms: f64,
    forecast: PlaceForecast,
}

fn is_fresh(fetched_at_ms: f64, now_ms: f64) -> bool {
    now_ms - fetched_at_ms < TTL_MS
}

/// One shared upstream HTTP client (reqwest clients are Arcs inside).
fn http() -> reqwest::Client {
    thread_local! {
        static HTTP: reqwest::Client = reqwest::Client::new();
    }
    HTTP.with(Clone::clone)
}

/// Geocode with a permanent per-name cache (a city's coordinates don't
/// move; the resolved display name is worth keeping offline).
async fn place(location: &str) -> Result<Place, String> {
    let key = format!("geocode:{}", location.trim().to_lowercase());
    if let Some(hit) = crate::offline::cache_get::<Place>(&key) {
        return Ok(hit);
    }
    let place = open_meteo::geocode(&http(), location).await?;
    crate::offline::cache_put(&key, &place);
    Ok(place)
}

/// The one weather read path: fresh cache → cached copy; otherwise fetch
/// and overwrite; on fetch failure serve the stale copy if there is one.
/// `now_index` is recomputed on EVERY cached read so a forecast fetched
/// hours ago still points at the current hour.
pub(crate) async fn place_weather(location: &str) -> Result<WeatherData, String> {
    let place = place(location).await?;
    let key = format!("weather:{}", place.name.to_lowercase());
    let now_ms = js_sys::Date::now();

    let cached = crate::offline::cache_get::<CachedForecast>(&key);
    if let Some(hit) = &cached
        && is_fresh(hit.fetched_at_ms, now_ms)
    {
        return Ok(revalidated(hit.forecast.clone()));
    }
    match open_meteo::forecast(&http(), &place).await {
        Ok(forecast) => {
            crate::offline::cache_put(
                &key,
                &CachedForecast { fetched_at_ms: now_ms, forecast: forecast.clone() },
            );
            Ok(forecast.data)
        }
        Err(err) => match cached {
            Some(hit) => Ok(revalidated(hit.forecast)),
            None => Err(err),
        },
    }
}

/// A cached forecast with `now_index` moved to the current local hour.
fn revalidated(forecast: PlaceForecast) -> WeatherData {
    let mut data = forecast.data;
    data.now_index = open_meteo::recompute_now_index(
        &data.hourly,
        chrono::Utc::now(),
        forecast.utc_offset_seconds,
    );
    data
}
```

`js_sys::Date::now()` / `chrono::Utc::now()` under wasm: `Utc::now()` works on wasm with chrono's `wasmbind`/js feature — CHECK how the workspace builds chrono for wasm (calendar.rs already uses `Local`/now client-side, so the feature is in place; reuse whatever it does).

Location resolution for callers with no explicit place: device pref, else the dashboard layout's weather widget. Add:

```rust
/// The place to show when none is given: the device preference, else the
/// location of the dashboard's weather widget (from the cached layout, so
/// it also resolves offline).
pub(crate) async fn default_location() -> Option<String> {
    if let Some(pref) = crate::pref(crate::WEATHER_LOCATION_KEY) {
        return Some(pref);
    }
    let layout = crate::offline::cache_get::<chaos_domain::DashboardLayout>("dashboard")?;
    layout.columns.iter().flat_map(|c| &c.widgets).find_map(|w| match &w.widget {
        chaos_domain::Widget::Weather { location } => Some(location.clone()),
        _ => None,
    })
}
```

(Reading the layout cache directly is deliberate: the dashboard already keeps it warm, and weather must not depend on the server. If the cache is empty AND no pref is set, the weather page shows "Set a location in settings" — acceptable first-run state.)

- [ ] **Step 3: Rewire WeatherRow**

`pages/weather.rs:344-361` — replace the `client.weather(query)` resource:

```rust
let data = LocalResource::new(move || {
    let query = location.clone();
    async move {
        let place = match query.or(crate::weather_fetch::default_location().await) {
            Some(place) => place,
            None => return Err("no location set — add one in settings".to_string()),
        };
        crate::weather_fetch::place_weather(&place).await
    }
});
```

The error arm currently prints `err.to_string()` on `ClientError` — it is now already a `String`; adjust types. Remove the now-unused `use_client` import if nothing else in the row uses it.

- [ ] **Step 4: Rewire the dashboard weather widget**

In `DataWidget` (`dashboard.rs`), the Weather kind no longer goes through `widget_data`. In `WidgetView` (or at the top of `DataWidget`), branch weather off to a dedicated component:

```rust
Widget::Weather { location } => view! { <WeatherWidget location/> }.into_any(),
```

```rust
/// Weather is fetched directly from Open-Meteo (see weather_fetch) — the
/// only dashboard widget with no server dependency, so it keeps polling
/// even while the server is unreachable.
#[component]
fn WeatherWidget(location: String) -> impl IntoView {
    let data = crate::hooks::use_polled_resource_with(WIDGET_REFRESH, None, true, move || {
        let configured = location.clone();
        async move {
            let place = crate::pref(crate::WEATHER_LOCATION_KEY).unwrap_or(configured);
            crate::weather_fetch::place_weather(&place).await
        }
    });
    let data = Memo::new(move |_| data.get());
    view! {
        <section class="widget widget-weather">
            <h2>
                <a class="widget-title-link" href="/weather" title="Open weather">"Weather"</a>
            </h2>
            {move || match data.get() {
                None => view! { <p class="muted">"Loading…"</p> }.into_any(),
                Some(Ok(weather)) => view! { <WeatherView weather/> }.into_any(),
                Some(Err(err)) => view! { <p class="error">{err}</p> }.into_any(),
            }}
        </section>
    }
}
```

Then delete the Weather arms from `DataWidget`'s kind/title/`weather_location` plumbing (`dashboard.rs:160,169,180-193,217-219`) — `DataWidget` no longer handles weather.

- [ ] **Step 5: Test, build wasm, commit**

Run: `cargo test --workspace && cargo build -p chaos-ui --target wasm32-unknown-unknown && cargo clippy --workspace --all-targets -- -D warnings`

```bash
git add crates/chaos-ui && git -c commit.gpgsign=false commit -m "feat(ui): weather fetched directly from Open-Meteo with local caches"
```

---

### Task 3: Remove the server weather proxy

**Files:**
- Delete: `crates/chaos-server/src/widgets/weather.rs`
- Modify: `crates/chaos-server/src/widgets/mod.rs` (drop `mod weather`, the `geocode` field, `WidgetHub::weather()`, the `Widget::Weather` fetch arm and its TTL arm)
- Modify: `crates/chaos-server/src/api/mod.rs` + the handler file containing the weather route (grep `api/v1/weather` / `weather` in `crates/chaos-server/src/api/`)
- Modify: `crates/chaos-domain/src/dashboard.rs` (drop `Widget::Weather` from `has_data()`)
- Modify: `crates/chaos-client/src/lib.rs` (remove `weather()` method and the `WeatherData` import)

- [ ] **Step 1: Domain — weather is no longer a data widget**

In `has_data()` (`dashboard.rs:137-148`) remove `Widget::Weather { .. }` from the matches! list and move it to the doc comment:

```rust
/// Whether this widget has a server-side data payload (`WidgetData`).
/// Weather is NOT one: clients fetch Open-Meteo directly.
```

Consequence: no `entries` registration → `GET /api/v1/widgets/{id}` answers 404/UnknownWidget for weather instances, which no client calls anymore. The layout still carries `Widget::Weather { location }` so clients know what to render — nothing else changes on the wire.

- [ ] **Step 2: Server — delete the module and its wiring**

In `widgets/mod.rs`:
- remove `mod weather;`, the `geocode: StaleCache<String, weather::Place>` field, `GEOCODE_CACHE_ENTRIES`, and their `new()` init;
- remove `WidgetHub::weather()` (L143-169);
- in `fetch()` move `Widget::Weather { .. }` into the "no data endpoint" arm;
- in `ttl()` move `Widget::Weather` into the `Duration::ZERO` arm;
- in `data()` the `location` query filtering referenced `Widget::Weather` (L95) — the `location` parameter is now dead: remove it from `data()`, from the widgets API handler (`api/widgets.rs` — the `?location=` query extraction), and from `ChaosClient::widget_data` (drop the parameter; fix the two UI call sites, which pass `None` after Task 2).
- Delete `widgets/weather.rs` (`git rm`).

Remove the weather route: grep `route(` in `api/mod.rs` for `/weather`, delete route + handler + any `WeatherQuery` param struct. Update `api/search.rs` if it referenced weather (it doesn't — verify with grep).

- [ ] **Step 3: Client — remove the proxy method**

Remove `ChaosClient::weather()` (`chaos-client/src/lib.rs:103-109`) and `WeatherData` from the import list.

- [ ] **Step 4: Full workspace green, commit**

Run: `cargo test --workspace && cargo build -p chaos-ui --target wasm32-unknown-unknown && cargo build -p chaos-web --target wasm32-unknown-unknown && cargo clippy --workspace --all-targets -- -D warnings`
Expected: green — any leftover reference the compiler finds, fix as part of this task.

```bash
git add -A && git -c commit.gpgsign=false commit -m "feat(server)!: remove the weather proxy — clients fetch Open-Meteo directly"
```

---

### Task 4: Posts sorted by upvotes (server + shared parsing in chaos-client)

**Files:**
- Modify: `crates/chaos-server/src/widgets/posts.rs`
- Create: `crates/chaos-client/src/posts.rs`
- Modify: `crates/chaos-client/src/lib.rs` (`pub mod posts;`)
- Modify: `crates/chaos-client/Cargo.toml` (add `futures` if missing)

- [ ] **Step 1: Server sorting, test-first**

In `widgets/posts.rs` tests:

```rust
#[test]
fn feeds_are_ordered_by_score_descending_scoreless_last() {
    let mut items = vec![
        FeedItem { title: "low".into(), score: Some(3), ..blank_item() },
        FeedItem { title: "none".into(), score: None, ..blank_item() },
        FeedItem { title: "high".into(), score: Some(90), ..blank_item() },
    ];
    sort_by_score(&mut items);
    let titles: Vec<_> = items.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles, ["high", "low", "none"]);
}
```

with a `fn blank_item() -> FeedItem` helper (all fields None/empty). Run to see it fail, then implement in posts.rs:

```rust
/// Once gathered, both aggregators are shown by upvotes, not by their
/// route's own ranking (still `topstories`/`hottest` — not the "best"
/// endpoints). Stable, so equal scores keep the upstream order.
fn sort_by_score(items: &mut [FeedItem]) {
    items.sort_by(|a, b| b.score.cmp(&a.score));
}
```

Call it in `hacker_news()` right before the `items.is_empty()` check (make `items` mut) and in `lobsters()` on the collected vec before wrapping in `WidgetData::Feed`.

- [ ] **Step 2: chaos-client posts module, test-first**

`crates/chaos-client/src/posts.rs` — the direct-fetch twin used by clients when the server is unreachable. Port the item-mapping from the server module (same `HnItem`/`LobstersStory` structs, `hn_item`/`lobsters_item` functions, same doc comments about text posts) — port their four unit tests too — plus:

```rust
//! Direct link-aggregator access for offline use: when the chaos server is
//! unreachable the dashboard fetches HN itself (their API sends CORS `*`)
//! and parses lobsters JSON fetched through the Tauri HTTP plugin
//! (lobste.rs sends no CORS headers, so browsers can't fetch it — the UI
//! passes the raw text in from `window.__TAURI__.http.fetch`).
//! The server has its own copy of this mapping in widgets/posts.rs — kept
//! separate because the server must not depend on this crate.

/// HN front page via the Firebase API; sorted by upvotes.
pub async fn hacker_news(http: &reqwest::Client, limit: u32) -> Result<Vec<FeedItem>, String>;

/// Parse a `hottest.json` body (fetched by the caller); sorted by upvotes.
pub fn parse_lobsters(json: &str, limit: u32) -> Result<Vec<FeedItem>, String>;

/// Upvotes descending, scoreless items last; stable.
pub fn sort_by_score(items: &mut [FeedItem]);
```

`hacker_news` mirrors the server implementation (take `limit` ids, `futures::future::join_all`, drop failed items, error when empty) with the same 8s-per-request deadline pattern as `open_meteo::get_json` — extract that helper into a `pub(crate) fn` shared inside chaos-client (e.g. `crate::http_get_json`) rather than duplicating it. `parse_lobsters` is `serde_json::from_str::<Vec<LobstersStory>>` + take(limit) + map + sort. Both end with `sort_by_score`.

New tests: the sort test (same shape as the server one) and

```rust
#[test]
fn parsed_lobsters_come_out_by_score() {
    let raw = r#"[
        {"short_id":"a","title":"low","url":"https://e.org/1","score":2,"comment_count":0,"comments_url":"https://lobste.rs/s/a","created_at":"2026-07-06T01:02:03Z"},
        {"short_id":"b","title":"high","url":"https://e.org/2","score":50,"comment_count":1,"comments_url":"https://lobste.rs/s/b","created_at":"2026-07-06T01:02:03Z"}
    ]"#;
    let items = parse_lobsters(raw, 10).unwrap();
    assert_eq!(items[0].title, "high");
}
```

- [ ] **Step 3: Run, commit**

Run: `cargo test -p chaos-server posts && cargo test -p chaos-client posts && cargo build -p chaos-client --target wasm32-unknown-unknown && cargo clippy --workspace --all-targets -- -D warnings`

```bash
git add -A && git -c commit.gpgsign=false commit -m "feat: HN/lobsters ordered by upvotes; direct posts module in chaos-client"
```

---

### Task 5: tauri-plugin-http + offline direct-feed path in the dashboard

**Files:**
- Modify: `crates/chaos-desktop/Cargo.toml`, `crates/chaos-desktop/src/lib.rs`, `crates/chaos-desktop/capabilities/*.json` (find the actual capability file: `ls crates/chaos-desktop/capabilities/`)
- Create: `crates/chaos-ui/src/tauri_http.rs`
- Modify: `crates/chaos-ui/src/lib.rs` (`mod tauri_http;`), `crates/chaos-ui/src/pages/dashboard.rs`

- [ ] **Step 1: Register the plugin**

`crates/chaos-desktop/Cargo.toml`: add `tauri-plugin-http = "2"`. In the builder chain in `src/lib.rs` (near `.invoke_handler`): `.plugin(tauri_plugin_http::init())`.

Capability file — add to the existing default capability's `permissions` array:

```json
{
  "identifier": "http:default",
  "allow": [
    { "url": "https://hacker-news.firebaseio.com/*" },
    { "url": "https://lobste.rs/*" },
    { "url": "https://api.open-meteo.com/*" },
    { "url": "https://geocoding-api.open-meteo.com/*" }
  ]
}
```

Open-Meteo hosts are included so the shells' weather requests also work if webview CORS ever regresses; harmless otherwise. `withGlobalTauri` is already true (`tauri.conf.json:12`) so `window.__TAURI__.http.fetch` will exist.

Note: `cargo check -p chaos-desktop` needs the dist placeholder (see memory: tauri needs `crates/chaos-web/dist` to exist — `mkdir -p crates/chaos-web/dist` if a build complains).

- [ ] **Step 2: The JS-interop fetch helper**

`crates/chaos-ui/src/tauri_http.rs`:

```rust
//! Native-fetch bridge: `window.__TAURI__.http.fetch` routes through the
//! shell's Rust HTTP client, sidestepping webview CORS. Only used for hosts
//! that don't send CORS headers (lobste.rs); scoped by the shell's
//! capability file to an explicit allowlist.

use leptos::wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

/// True when running inside a shell that exposes the HTTP plugin.
pub(crate) fn available() -> bool {
    fetch_fn().is_some()
}

fn fetch_fn() -> Option<js_sys::Function> {
    let window = web_sys::window()?;
    let tauri = js_sys::Reflect::get(&window, &"__TAURI__".into()).ok()?;
    if tauri.is_undefined() {
        return None;
    }
    let http = js_sys::Reflect::get(&tauri, &"http".into()).ok()?;
    js_sys::Reflect::get(&http, &"fetch".into())
        .ok()?
        .dyn_into()
        .ok()
}

/// GET `url` through the shell and return the body text. `None` when no
/// plugin is available (plain browser); `Some(Err)` on request failure.
pub(crate) async fn fetch_text(url: &str) -> Option<Result<String, String>> {
    let fetch = fetch_fn()?;
    Some(fetch_text_inner(&fetch, url).await)
}

async fn fetch_text_inner(fetch: &js_sys::Function, url: &str) -> Result<String, String> {
    let promise: js_sys::Promise = fetch
        .call1(&leptos::wasm_bindgen::JsValue::UNDEFINED, &url.into())
        .map_err(|e| format!("{e:?}"))?
        .dyn_into()
        .map_err(|_| "fetch did not return a promise".to_string())?;
    let response: web_sys::Response = JsFuture::from(promise)
        .await
        .map_err(|e| format!("{e:?}"))?
        .dyn_into()
        .map_err(|_| "not a Response".to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let text = JsFuture::from(response.text().map_err(|e| format!("{e:?}"))?)
        .await
        .map_err(|e| format!("{e:?}"))?;
    text.as_string().ok_or_else(|| "body was not text".into())
}
```

Check chaos-ui's Cargo.toml for `wasm-bindgen-futures`, `js-sys`, and the web-sys `Response` feature — add what's missing (web-sys features: `Response`).

- [ ] **Step 3: Offline direct path for the two posts widgets**

In `DataWidget` (`dashboard.rs`), HN and Lobsters get the same treatment weather got — but only offline; online they keep the server cache. In the fetch closure, branch:

```rust
// HN/lobsters can be fetched without the server: HN's API sends CORS,
// lobsters only works through the shell's HTTP plugin. Cached under the
// same widget key either way, so each path serves the other's leftovers.
let direct = match &widget {
    Widget::HackerNews { limit, .. } => Some(DirectFeed::HackerNews(*limit)),
    Widget::Lobsters { limit, .. } => Some(DirectFeed::Lobsters(*limit)),
    _ => None,
};
```

with

```rust
#[derive(Clone, Copy)]
enum DirectFeed {
    HackerNews(u32),
    Lobsters(u32),
}

impl DirectFeed {
    async fn fetch(self) -> Result<WidgetData, chaos_client::ClientError> {
        use chaos_client::ClientError;
        let items = match self {
            DirectFeed::HackerNews(limit) => {
                chaos_client::posts::hacker_news(&crate::weather_fetch::http(), limit).await
            }
            DirectFeed::Lobsters(limit) => match crate::tauri_http::fetch_text(
                "https://lobste.rs/hottest.json",
            )
            .await
            {
                Some(Ok(json)) => chaos_client::posts::parse_lobsters(&json, limit),
                Some(Err(err)) => Err(err),
                None => Err("lobsters needs the app shell offline".into()),
            },
        }
        .map_err(ClientError::Transport)?;
        Ok(WidgetData::Feed { items })
    }
}
```

(make `weather_fetch::http()` `pub(crate)`). The resource closure becomes: online → `cached(conn, widget:{id}, client.widget_data(..))` as in the offline-core plan; offline with `direct` → `cached(conn, widget:{id}, direct.fetch())` — a direct success overwrites the widget cache so a later web-offline lobsters view has fresh leftovers; `cached` still serves stale on failure and can't downgrade conn further. Use `use_polled_resource_with(WIDGET_REFRESH, None, direct.is_some(), …)` so these two widgets keep their 300s cadence offline while everything else pauses.

The "· cached" stale hint from the offline-core plan must NOT show for a fresh direct fetch (stale flag false) — it comes through `cached()` correctly; just verify.

- [ ] **Step 4: Full gates + desktop check, commit**

Run: `cargo test --workspace && cargo build -p chaos-ui --target wasm32-unknown-unknown && cargo build -p chaos-web --target wasm32-unknown-unknown && cargo clippy --workspace --all-targets -- -D warnings && cargo check -p chaos-desktop`

```bash
git add -A && git -c commit.gpgsign=false commit -m "feat: HN/lobsters fetched directly when offline (tauri http for lobsters)"
```

---

### Task 6: Docs + example config

**Files:**
- Modify: `docs/ROADMAP.md`, `docs/deployment.md`, `crates/chaos-server/chaos.example.toml`

- [ ] **Step 1: Update and commit**

- `chaos.example.toml`: the `[[columns.widgets]] type = "weather"` entry keeps working — update its comment: location is geocoded by the CLIENTS via Open-Meteo now; the server does not fetch weather.
- `docs/deployment.md`: note the removed `/api/v1/weather` endpoint (breaking, clients ship together) and the new Tauri http permissions for the shells (next APK build picks them up).
- `docs/ROADMAP.md`: tick/append offline + direct-fetch entries.

```bash
git add -A && git -c commit.gpgsign=false commit -m "docs: direct weather/posts fetching and offline behavior"
```
