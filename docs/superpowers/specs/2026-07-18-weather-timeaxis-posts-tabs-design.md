# Weather time-axis alignment + HN/lobsters time-range tabs — Design

Date: 2026-07-18. Approved by Tibo (AskUserQuestion): viewer-local time axis
everywhere; day bands + weekday in tooltip (emoji move from axis labels to
tooltip); true window-tops via archive APIs (HN Algolia, lobsters newest);
combined view default and rendered on top.

## Problems

1. **Timezone offset on the weather page.** Open-Meteo hourly series are in
   *location-local* time starting at local midnight 16 days back. Charts use a
   category x-axis and align rows **by array index**, so places in different
   timezones pair different real instants: the combined chart's lines are
   shifted against each other, split-view zoom sync (ECharts connect group,
   percent-based) misaligns, and "now" markers sit at different x positions.
2. **Days are hard to distinguish** in the 32-day hourly charts — only a
   "Fri 10" label at midnight, easily thinned away by `hideOverlap`.
3. Combined view is opt-in per visit and renders *below* the location rows.
4. HN/lobsters widgets show one live ranking; no way to see the top links of
   the last 24 h / 48 h / week.

## Part 1 — Weather charts on a time axis

### Data plumbing

- `chaos-ui/src/weather_fetch.rs`: `place_weather` currently returns
  `WeatherData`, discarding `PlaceForecast::utc_offset_seconds`. It now
  returns both (a small struct or tuple `(WeatherData, i32)`); the TTL cache
  already stores the full `PlaceForecast`, so no cache format change.
- `weather.rs`'s `LoadedForecasts` entries gain the offset:
  `(name, hourly, now_index, utc_offset_seconds)` (now_index retained for the
  default zoom window).

### Chart changes (`chaos-ui/src/pages/weather.rs`)

- Split **and** combined charts switch `xAxis.type` from `category` to
  `time`. Each series point becomes `[utc_epoch_ms, temp]`, where
  `utc_epoch_ms = (local_time - utc_offset_seconds).timestamp_millis()`.
  ECharts renders time-axis labels in the **viewer's** local time, so the
  same x position is the same real instant in every chart.
- All charts pin `xAxis.min`/`xAxis.max` to the union span over every loaded
  place (each chart also folds in its own series, as y_range does today).
  Identical spans make the percent-based zoom sync of the `weather` connect
  group align exactly.
- "now" markLine: `xAxis` = current UTC epoch millis (passed in by the
  caller; option builders stay pure). One real shared "now" across charts.
  Split charts keep their markLine; combined keeps it on the first series.
- `default_window(now_ms, min_ms, max_ms)` replaces the index-based version:
  past 24 h → next 48 h as percent of `[min_ms, max_ms]`, clamped.
  Double-click reset unchanged.
- **Day bands:** a `markArea` (silent, on the first/only series) shading
  every *other* viewer-local calendar day across `[min_ms, max_ms]` with a
  subtle theme-aware color (border color at low alpha). Built in Rust
  (chrono + the viewer's UTC offset read from `js_sys::Date` at the call
  site, injected as a parameter so builders stay testable).
- **Tooltip:** new `chaosWeatherTooltip` window function in
  `chaos-web/index.html` (attached via the existing `tooltip_formatter`
  prop): header "Fri 18 Jul, 14:00" (viewer-local, from `axisValue`), body
  one line per series: colored marker, place name (shown in split view
  too — one shared formatter, and the split series is named after its
  place), weather emoji, temperature. Emoji rides as a third element of
  each data point (`[ts, temp, emoji]`); the formatter reads `p.value[2]`.
  Temperatures display as `20.1°` — the letter unit stays on the y-axis
  labels, as today.
- Axis emoji labels (`hourly_labels`) and single-line `time_labels` are
  deleted; ECharts' automatic time labels (day/date at midnight, hours
  between) replace them.

### View & layout

- `combined` defaults to **true** and is persisted as a device pref
  (localStorage key `chaos-weather-combined`, existing pref helpers).
- The combined chart section renders **above** the location rows. Rows always
  show the current-conditions header (emoji, name, details, temp, remove
  button); their individual chart only mounts in split view (as today).

### Testing

Pure-function tests updated/added: utc conversion of series points, shared
axis min/max union, ms-based default window, day-band generation (band
boundaries at viewer-local midnights, alternating), option JSON assertions
(axis type "time", min/max set, markArea present, emoji in data triples).

## Part 2 — HN/lobsters time-range tabs

### Domain (`chaos-domain/src/dashboard.rs`)

```rust
/// Top links per trailing time window, each sorted by upvotes desc.
pub struct PostsData {
    pub last_24h: Vec<FeedItem>,
    pub last_48h: Vec<FeedItem>,
    pub last_week: Vec<FeedItem>,
}
WidgetData::Posts(PostsData)   // new variant; Feed stays for RSS widgets
```

Breaking wire change for HN/Lobsters widget payloads — fine, server and
clients ship together.

### Server (`chaos-server/src/widgets/posts.rs`)

- **HN via Algolia** (`https://hn.algolia.com/api/v1/search`): one request
  per window — `?tags=front_page&numericFilters=created_at_i>{cutoff}&hitsPerPage=50`.
  Map hits (`title`, `url` nullable → discussion link
  `https://news.ycombinator.com/item?id={objectID}`, `points` → score,
  `num_comments` → comments, `created_at_i` → published). Sort by score
  desc (existing `sort_by_score`), truncate to the widget `limit`.
  The Firebase topstories path is removed.
- **Lobsters via `https://lobste.rs/newest.json?page=N`**: fetch pages
  starting at 1 until the page's oldest story is older than the 7-day cutoff
  or 10 pages (cap; ~25 stories/page ≈ 1.3 days/page). Bucket stories into
  the three windows by `created_at`, sort each by score, truncate to
  `limit`. `hottest.json` path removed.
- Existing 10-minute per-widget response cache covers the extra upstream
  requests (≤ 3 Algolia + ≤ 10 lobsters calls per refresh).

### Client direct fetch (`chaos-client/src/posts.rs`)

Mirrors the server mapping (same reason as today: server must not depend on
chaos-client):

- `hacker_news(http, limit, now)` does the three Algolia calls with reqwest —
  Algolia sends `Access-Control-Allow-Origin: *` (verified), so it works in
  browsers and Tauri alike. Returns `PostsData`.
- `parse_lobsters(pages: &[String], limit, now)` takes the raw page bodies
  (fetched by the UI through the Tauri HTTP plugin, page by page with the
  same stop conditions) and buckets/sorts into `PostsData`.
- `now: DateTime<Utc>` is a parameter so cutoff math is testable.

### UI (`chaos-ui/src/pages/dashboard.rs`)

- Widgets whose data is `WidgetData::Posts` render a small tab strip
  (Last 24h · 48h · Week) above the list; an `RwSignal<Tab>` per widget,
  default Last 24h, not persisted. Tab switch is pure UI.
- List rendering reuses `feed_item_view` unchanged.
- Offline: the whole `PostsData` payload rides the existing `widget:{id}`
  cache and the `DirectFeed` offline fetchers return the same shape, so all
  three tabs work offline (lobsters: only in Tauri, as today).

### Tauri capability (`chaos-desktop/capabilities/default.json`)

Add `https://hn.algolia.com/*` to the http:default scope; drop
`https://hacker-news.firebaseio.com/*` (no longer called). Regenerate
desktop schemas; android/mobile schemas regenerate on next android build.

### Empty windows

A window with no stories (rare; lobsters 24h on a slow day) renders the
tab strip over an empty list — the widgets' existing empty rendering.
Not an error.

### Testing

Mapping tests for Algolia hits (nullable url → discussion), lobsters
bucketing across cutoffs, stop-condition logic (cutoff page / cap), sorting
+ truncation per window, and a UI-side tab-selection option test if cheap.

## Docs

`docs/ROADMAP.md` note; `docs/deployment.md`: HN/lobsters upstreams changed
to hn.algolia.com and lobste.rs/newest (firewall/egress note), weather page
now viewer-local.
