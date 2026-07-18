# Weather Time-Axis Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Align the weather page's charts by real instant (viewer-local time axis), add alternating day bands + a rich tooltip, and make the combined view the persisted default rendered above the rows.

**Architecture:** Charts switch from a category x-axis (index-aligned, location-local labels) to an ECharts `time` axis where every point is `[utc_epoch_ms, temp, emoji]`. `utc_offset_seconds` (already cached in `PlaceForecast` but currently dropped) flows from `weather_fetch` into the page. All charts pin `xAxis.min/max` to the union span of loaded places so the percent-based zoom sync of the `weather` connect group aligns exactly. A `chaosWeatherTooltip` window function (same mechanism as home.rs's `chaosTimeTooltip`) renders the weekday/date header and per-place emoji+temp lines.

**Tech Stack:** Leptos 0.8 CSR, ECharts via `chaos-ui/src/echarts.rs` JSON bridge (has `tooltip_formatter` window-fn prop), chrono, serde_json. Spec: `docs/superpowers/specs/2026-07-18-weather-timeaxis-posts-tabs-design.md`.

**Verification commands (used by every task):**
- `cargo test -p chaos-ui`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`
- wasm build check: `cargo check -p chaos-ui --target wasm32-unknown-unknown`

---

### Task A1: Plumb `utc_offset_seconds` to the weather page

**Files:**
- Modify: `crates/chaos-ui/src/weather_fetch.rs` (place_weather return type)
- Modify: `crates/chaos-ui/src/pages/weather.rs` (LoadedForecasts → struct with offset)
- Modify: `crates/chaos-ui/src/pages/dashboard.rs` (WeatherWidget call site ignores offset)

- [ ] **Step 1: Change `place_weather` to keep the offset.** Return `Result<(WeatherData, i32), String>` (the `i32` is `utc_offset_seconds`). The fresh-cache path returns `(revalidated(hit.forecast.clone()), hit.forecast.utc_offset_seconds)` (bind the offset before moving the forecast); the fetch path returns `(forecast.data, forecast.utc_offset_seconds)` after `cache_put`; the stale-fallback path likewise. Keep `revalidated` as-is internally (it may now take `PlaceForecast` and return the pair — pick the cleanest shape).

- [ ] **Step 2: Adapt the dashboard widget.** In `crates/chaos-ui/src/pages/dashboard.rs`, `WeatherWidget`'s resource calls `place_weather`; map the result with `.map(|(data, _)| data)` — the dashboard card doesn't need the offset.

- [ ] **Step 3: Replace the `LoadedForecasts` tuple with a struct** in `weather.rs`:

```rust
/// One loaded location published into the page-wide list.
#[derive(Clone)]
struct LoadedPlace {
    name: String,
    hourly: Vec<chaos_domain::HourlyForecast>,
    now_index: usize,
    utc_offset_seconds: i32,
}
type LoadedForecasts = Vec<LoadedPlace>;
```

Update every construction/pattern-match site (`WeatherRow` publish effect, `on_cleanup` retract, `y_range` callers, `CombinedChart`'s `first` memo, tests' `two_locations`). `WeatherRow`'s `LocalResource` now yields the `(WeatherData, i32)` pair; the publish Effect stores the offset; `weather_row_body` receives both.

- [ ] **Step 4: Run `cargo test -p chaos-ui` and the wasm check.** Expected: compiles, existing chart tests still pass (they'll be reworked in A2 — adjust constructors only, keep assertions).

- [ ] **Step 5: Commit** `feat(weather): carry utc_offset_seconds to the weather page` (unsigned: `git -c commit.gpgsign=false commit`).

---

### Task A2: Time-axis charts with day bands and tooltip

**Files:**
- Modify: `crates/chaos-ui/src/pages/weather.rs` (option builders + tests)
- Modify: `crates/chaos-web/index.html` (add `chaosWeatherTooltip`)

- [ ] **Step 1: Write failing tests first** in `weather.rs` `mod tests` (delete `labels_show_*` and `combined_labels_have_no_emoji`; rework option tests). New pure helpers to test:

```rust
/// Milliseconds since epoch of a location-local forecast hour.
fn utc_ms(local: chrono::NaiveDateTime, utc_offset_seconds: i32) -> i64 {
    (local - chrono::Duration::seconds(utc_offset_seconds as i64))
        .and_utc()
        .timestamp_millis()
}

/// `[ts, temp, emoji]` triples for a series.
fn series_points(
    hourly: &[chaos_domain::HourlyForecast],
    utc_offset_seconds: i32,
    fahrenheit: bool,
) -> Vec<serde_json::Value>

/// Union [min, max] in ms over every loaded place (plus own row's data).
fn axis_span(all: &LoadedForecasts, own: Option<(&[HourlyForecast], i32)>) -> Option<(i64, i64)>

/// Default window (now-24h → now+48h) as percent of [min, max], clamped.
fn default_window_ms(now_ms: i64, min_ms: i64, max_ms: i64) -> (f64, f64)

/// Alternating viewer-local day bands over [min, max]:
/// [[{xAxis: start_ms}, {xAxis: end_ms}], …] for every OTHER calendar day.
/// `viewer_offset_seconds` = -(js Date.getTimezoneOffset() * 60) at call
/// sites; a parameter so this stays testable. DST shifts inside the 32-day
/// span move band edges by an hour — acceptable.
fn day_bands(min_ms: i64, max_ms: i64, viewer_offset_seconds: i32) -> serde_json::Value
```

Tests: `utc_ms` for a +2 h offset (local 14:00 → 12:00 UTC epoch); `series_points` carries `[ts, temp, emoji-string]`; `axis_span` union across two offset places; `default_window_ms` centred/clamped cases; `day_bands` starts at the viewer-local midnight at-or-before `min_ms`, alternates (bands 0, 2, 4… shaded), last band clipped to `max_ms`; option assertions below.

- [ ] **Step 2: Run tests, verify they fail** (`cargo test -p chaos-ui`).

- [ ] **Step 3: Rewrite the option builders.** Both builders take `now_ms: i64` and `viewer_offset_seconds: i32` parameters (callers pass `js_sys::Date::now() as i64` and `-(js_sys::Date::new_0().get_timezone_offset() as i32) * 60`; keep builders DOM-free).

Split (`weather_chart_option(own: &LoadedPlace-ish, all, now_ms, viewer_offset_seconds, fahrenheit, colors)`):
- `xAxis`: `{"type": "time", "min": min_ms, "max": max_ms, "axisLabel": {"color": muted, "hideOverlap": true}, "axisLine": …}` — no `data`, no category labels.
- one series, **named after the location** (the tooltip shows the name), `"data": series_points(...)`.
- `markLine` at `{"xAxis": now_ms}`.
- `markArea` on the series: `{"silent": true, "itemStyle": {"color": border, "opacity": 0.08}, "data": day_bands(min_ms, max_ms, viewer_offset_seconds)}`.
- everything else (tooltip surface colors, `inside_zoom`, pinned y-range via existing `y_range`) unchanged.

Combined (`combined_chart_option(all, now_ms, viewer_offset_seconds, fahrenheit, colors)`):
- same time axis pinned to `axis_span(all, None)`; one series per place with `series_points` using each place's own offset (this is the actual bug fix — no more borrowing the first place's timestamps); markLine `{"xAxis": now_ms}` and the markArea both on the first series; legend unchanged.
- delete `hourly_labels`, `time_labels`, and the old `default_window`.

Option test assertions: `xAxis.type == "time"`, min/max equal the union span, series data points are `[ts, temp, emoji]` with correct ts for a non-zero offset, markLine at `now_ms`, markArea present with alternating pairs, split option has exactly one series named after the place, combined has one per place with distinct colors.

- [ ] **Step 4: Wire the components.** `weather_row_body` and `CombinedChart` compute `now_ms`/`viewer_offset_seconds` (browser-only, outside the pure builders), pass them in, use `default_window_ms` for `reset_zoom`, and set `tooltip_formatter="chaosWeatherTooltip"` on both `ChartCanvas` calls. `CombinedChart`'s remount `Memo` keys on the first place's `(hourly.len(), now_index)` as today.

- [ ] **Step 5: Add the tooltip formatter** to `crates/chaos-web/index.html` next to `chaosTimeTooltip`:

```js
// Weather charts: header = viewer-local weekday/date/hour of the hovered
// instant; one line per place with its weather emoji. value = [ts, temp, emoji].
window.chaosWeatherTooltip = function (params) {
  if (!Array.isArray(params) || params.length === 0) return "";
  var d = new Date(params[0].value[0]);
  var days = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
  var months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
                "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
  var pad = function (n) { return (n < 10 ? "0" : "") + n; };
  var header = days[d.getDay()] + " " + d.getDate() + " " + months[d.getMonth()]
    + ", " + pad(d.getHours()) + ":" + pad(d.getMinutes());
  var lines = params
    .filter(function (p) { return p.value != null && p.value[1] != null; })
    .map(function (p) {
      var emoji = p.value[2] ? p.value[2] + " " : "";
      return p.marker + " " + p.seriesName + "  " + emoji + "<b>" + p.value[1] + "°</b>";
    });
  return "<div>" + header + "</div>" + lines.join("<br/>");
};
```

- [ ] **Step 6: Run all verification commands.** Expected: green.

- [ ] **Step 7: Commit** `feat(weather): time-axis charts aligned by real instant, day bands, rich tooltip`.

---

### Task A3: Combined view default, persisted, on top

**Files:**
- Modify: `crates/chaos-ui/src/pages/weather.rs` (WeatherPage)
- Modify: `crates/chaos-web/styles.css` (only if spacing needs it)

- [ ] **Step 1: Persist the toggle.** Use the existing device-pref helpers in `chaos-ui/src/lib.rs` (the same localStorage-backed `pref`/set mechanism `weather_fahrenheit`/`weather_places` use — follow their exact pattern, adding a helper pair if that's the convention). Key: `weather-combined`. Default **true** when unset. `combined` starts from the stored value; the toggle button writes it back on every click.

- [ ] **Step 2: Reorder the page.** In `WeatherPage`'s view, move the `<Show when=combined><CombinedChart loaded/></Show>` block ABOVE the rows (`{move || { let list = places.get(); … }}` block). Rows keep rendering their current-conditions header always; their split chart already hides when `combined` (existing `<Show when=!combined>`).

- [ ] **Step 3: Manual sanity + tests.** `cargo test -p chaos-ui`, wasm check, clippy, fmt.

- [ ] **Step 4: Commit** `feat(weather): combined view is the persisted default, rendered first`.
