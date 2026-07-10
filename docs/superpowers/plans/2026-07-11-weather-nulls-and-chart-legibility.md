# Weather null-tolerance + Home chart legibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the recurring weather 502s (null entries in Open-Meteo's forecast tail), surface decode-error detail in chaos-client (mobile lights diagnosis), default the Home temperature chart to 7 days zoomed on today, and make days legible on the chart axis + tooltip.

**Architecture:** chaos-server's weather provider deserializes Open-Meteo JSON (crates/chaos-server/src/widgets/weather.rs); chaos-client wraps reqwest errors; the Home chart builds ECharts options as JSON through crates/chaos-ui/src/echarts.rs `ChartCanvas`.

**Branch:** `fix/weather-nulls-chart-legibility` off `develop`.

---

### Task A: Weather provider tolerates null series entries

Open-Meteo returns `null` in `hourly.temperature_2m`/`hourly.weather_code` and `daily.*` for hours/days beyond its model horizon (observed: the last requested forecast day for Rennes; location/timezone dependent). `Vec<f64>` deserialization fails → "error decoding response body" → 502.

**Files:**
- Modify: `crates/chaos-server/src/widgets/weather.rs`

- [ ] **Step 1: Failing test first**

Add to the existing `#[cfg(test)] mod tests` in weather.rs a parse+build test using a fixture with nulls. The `Forecast` struct is module-private so test it directly:

```rust
    #[test]
    fn forecast_tolerates_null_tail_entries() {
        // Open-Meteo leaves nulls where its model horizon ends (observed on
        // the 16th forecast day for some locations/timezones).
        let raw = r#"{
            "current": {"time": "2026-07-11T00:00", "temperature_2m": 28.0,
                        "apparent_temperature": 28.1, "relative_humidity_2m": 44,
                        "weather_code": 0, "wind_speed_10m": 9.5},
            "daily": {"time": ["2026-07-11", "2026-07-12"],
                      "weather_code": [3, null],
                      "temperature_2m_max": [30.0, null],
                      "temperature_2m_min": [18.0, null]},
            "hourly": {"time": ["2026-07-11T00:00", "2026-07-11T01:00", "2026-07-11T02:00"],
                       "temperature_2m": [20.0, null, 21.0],
                       "weather_code": [0, null, 1]}
        }"#;
        let forecast: super::Forecast = serde_json::from_str(raw).expect("nulls must parse");
        let (daily, hourly) = super::build_series(forecast.daily, forecast.hourly);
        // Null-bearing entries are dropped, complete ones kept.
        assert_eq!(daily.len(), 1);
        assert_eq!(daily[0].max_c, 30.0);
        assert_eq!(hourly.len(), 2);
        assert_eq!(hourly[1].temp_c, 21.0);
    }
```

Run: `nix develop -c cargo nextest run -p chaos-server forecast_tolerates` — must FAIL (types/function don't exist yet).

- [ ] **Step 2: Make series fields nullable and extract `build_series`**

In `HourlySeries`: `temperature_2m: Vec<Option<f64>>`, `weather_code: Vec<Option<i32>>`.
In `DailySeries`: `weather_code: Vec<Option<i32>>`, `temperature_2m_max: Vec<Option<f64>>`, `temperature_2m_min: Vec<Option<f64>>`.

Extract the zip/collect logic from `fetch` into a testable free function that drops incomplete entries:

```rust
/// Zip the raw Open-Meteo series into forecast entries, dropping any hour or
/// day with a null value (the model horizon's ragged edge — better a shorter
/// series than a failed fetch).
fn build_series(daily: DailySeries, hourly: HourlySeries) -> (Vec<DailyForecast>, Vec<HourlyForecast>) {
    let daily = daily
        .time
        .into_iter()
        .zip(daily.temperature_2m_min)
        .zip(daily.temperature_2m_max)
        .zip(daily.weather_code)
        .filter_map(|(((date, min_c), max_c), weather_code)| {
            Some(DailyForecast {
                date,
                min_c: min_c?,
                max_c: max_c?,
                weather_code: weather_code?,
            })
        })
        .collect();
    let hourly = hourly
        .time
        .into_iter()
        .zip(hourly.temperature_2m)
        .zip(hourly.weather_code)
        .filter_map(|((time, temp_c), weather_code)| {
            Some(HourlyForecast {
                time,
                temp_c: temp_c?,
                weather_code: weather_code?,
            })
        })
        .collect();
    (daily, hourly)
}
```

Rework `fetch` to call `build_series(forecast.daily, forecast.hourly)` and keep the existing `filter(date >= today)` on the daily result (order: build first, then the today filter — or fold the filter in; keep behavior identical otherwise). `now_index` computed on the filtered hourly as before.

- [ ] **Step 3: Tests green + live-shape verification**

Run: `nix develop -c cargo nextest run -p chaos-server` — all pass, including the new test and the existing `now_index_*` tests.

Also verify against the real API (the actual Rennes payload that 502s in prod):

```bash
curl -s "https://api.open-meteo.com/v1/forecast?latitude=48.11109&longitude=-1.67431&current=temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,wind_speed_10m&daily=weather_code,temperature_2m_max,temperature_2m_min&hourly=temperature_2m,weather_code&timezone=auto&forecast_days=16&past_days=16" | grep -c null
```
(nonzero nulls expected — the fixture mirrors reality).

- [ ] **Step 4: Commit**

```bash
git add crates/chaos-server/src/widgets/weather.rs
git commit -m "fix(weather): tolerate null entries at open-meteo's forecast horizon"
```

---

### Task B: chaos-client decode errors carry the underlying cause

`ClientError::Decode` currently stores `e.to_string()` of the reqwest error, which is just "error decoding response body" — the serde/body detail lives in the error's source chain and is dropped. The mobile lights bug is undiagnosable because of this.

**Files:**
- Modify: `crates/chaos-client/src/lib.rs`

- [ ] **Step 1: Error-chain helper + use it**

```rust
/// Flatten an error and its source chain into one line ("outer: inner:
/// innermost"), because ClientError is stringly-typed and `Display` on
/// reqwest errors hides the serde detail that actually names the problem.
fn error_chain(e: impl std::error::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(inner) = source {
        out.push_str(": ");
        out.push_str(&inner.to_string());
        source = inner.source();
    }
    out
}
```

In `send()`: `.map_err(|e| ClientError::Decode(error_chain(e)))`.

- [ ] **Step 2: Unit test**

```rust
#[cfg(test)]
mod tests {
    use std::fmt;

    #[derive(Debug)]
    struct Outer(Inner);
    #[derive(Debug)]
    struct Inner;
    impl fmt::Display for Outer {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "error decoding response body")
        }
    }
    impl fmt::Display for Inner {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "missing field `on` at line 1 column 12")
        }
    }
    impl std::error::Error for Inner {}
    impl std::error::Error for Outer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn error_chain_includes_sources() {
        assert_eq!(
            super::error_chain(Outer(Inner)),
            "error decoding response body: missing field `on` at line 1 column 12"
        );
    }
}
```

Run: `nix develop -c cargo nextest run -p chaos-client` (if the crate has no test target wired, `cargo test -p chaos-client` inside nix; nextest handles it).

- [ ] **Step 3: Commit**

```bash
git add crates/chaos-client/src/lib.rs
git commit -m "fix(client): decode errors surface the underlying serde/body cause"
```

---

### Task C: Home temperature defaults to 7 days, zoomed on today

**Files:**
- Modify: `crates/chaos-ui/src/pages/home.rs`

- [ ] **Step 1: Default range + reset window**

In `HomePage`, the initial range becomes 7 days:

```rust
    let range = RwSignal::new((now - Duration::days(7), now));
```

In `TemperatureChart`, compute the initial/double-click zoom window as percent of the range covering today (local midnight → end) and pass it to ChartCanvas:

```rust
/// Percent window of `[start, end]` covering the current local day — the
/// chart's initial and double-click zoom. Falls back to the full range when
/// today's midnight precedes `start` (short custom ranges).
fn today_window(start: DateTime<Utc>, end: DateTime<Utc>) -> (f64, f64) {
    let midnight = Local::now().date_naive().and_hms_opt(0, 0, 0).unwrap();
    let Some(midnight) = Local.from_local_datetime(&midnight).single() else {
        return (0.0, 100.0);
    };
    let midnight = midnight.with_timezone(&Utc);
    let span = (end - start).num_milliseconds() as f64;
    if midnight <= start || span <= 0.0 {
        return (0.0, 100.0);
    }
    let pct = (midnight - start).num_milliseconds() as f64 / span * 100.0;
    (pct.min(100.0), 100.0)
}
```

In `TemperatureChart`'s view: `let reset = today_window(start, end);` then `<crate::echarts::ChartCanvas option reset_zoom=reset class="temp-chart"/>`.

- [ ] **Step 2: Unit tests**

`today_window` calls `Local::now()` — NOT unit-testable as-is. Split: `fn today_window_at(midnight_utc: DateTime<Utc>, start: .., end: ..) -> (f64, f64)` (pure) + a thin `today_window` wrapper supplying the real midnight. Test the pure part:

```rust
#[cfg(test)]
mod tests {
    use super::today_window_at;
    use chrono::{TimeZone, Utc};

    #[test]
    fn today_window_covers_the_last_day_of_a_week_range() {
        let start = Utc.with_ymd_and_hms(2026, 7, 4, 12, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2026, 7, 11, 12, 0, 0).unwrap();
        let midnight = Utc.with_ymd_and_hms(2026, 7, 11, 0, 0, 0).unwrap();
        let (from, to) = today_window_at(midnight, start, end);
        assert!((from - (6.5 / 7.0 * 100.0)).abs() < 0.01);
        assert_eq!(to, 100.0);
    }

    #[test]
    fn today_window_falls_back_to_full_range() {
        let start = Utc.with_ymd_and_hms(2026, 7, 11, 6, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2026, 7, 11, 18, 0, 0).unwrap();
        let midnight = Utc.with_ymd_and_hms(2026, 7, 11, 0, 0, 0).unwrap();
        assert_eq!(today_window_at(midnight, start, end), (0.0, 100.0));
    }
}
```

(If home.rs already has a tests module, extend it.) Run: `nix develop -c cargo nextest run -p chaos-ui today_window`.

- [ ] **Step 3: Check + commit**

`nix develop -c just check && nix develop -c just test`

```bash
git add crates/chaos-ui/src/pages/home.rs
git commit -m "feat(home): chart loads 7 days, zoomed on the current day"
```

---

### Task D: Day-legible axis labels + tooltip day header

**Files:**
- Modify: `crates/chaos-web/index.html` (tooltip formatter JS helper)
- Modify: `crates/chaos-ui/src/echarts.rs` (`ChartCanvas` attaches window JS functions into the option)
- Modify: `crates/chaos-ui/src/pages/home.rs` (leveled axis formatter + use the tooltip helper)

- [ ] **Step 1: Leveled x-axis label formatter (pure JSON)**

In `chart_option` (home.rs), the time xAxis's `axisLabel` gains a leveled template formatter — ECharts renders the coarsest applicable level at each tick, so day boundaries automatically show the day:

```rust
        "xAxis": {
            "type": "time",
            "min": start.timestamp_millis(),
            "max": end.timestamp_millis(),
            "axisLabel": {
                "color": muted,
                "hideOverlap": true,
                // Leveled templates: ticks at day/month boundaries name the
                // day, plain hours stay short — the day is always visible on
                // the axis without a function formatter.
                "formatter": {
                    "year": "{yyyy}",
                    "month": "{MMM} {d}",
                    "day": "{ee} {d}",
                    "hour": "{HH}:{mm}",
                    "minute": "{HH}:{mm}",
                },
            },
            "axisLine": { "lineStyle": { "color": border } },
            "splitLine": { "show": false },
        },
```

- [ ] **Step 2: Tooltip formatter JS helper**

ECharts can't take a tooltip formatter through JSON (functions only), so define one global helper in `crates/chaos-web/index.html`, right after the echarts `<script>` tag:

```html
    <script>
      // Axis-tooltip formatter for time-axis charts: header carries the full
      // day + time (the default header hides the date at hour granularity),
      // body lists each visible series with its colored marker.
      window.chaosTimeTooltip = function (params) {
        if (!Array.isArray(params) || params.length === 0) return "";
        var d = new Date(params[0].axisValue);
        var days = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        var months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
                      "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        var pad = function (n) { return (n < 10 ? "0" : "") + n; };
        var header = days[d.getDay()] + " " + d.getDate() + " " + months[d.getMonth()]
          + ", " + pad(d.getHours()) + ":" + pad(d.getMinutes());
        var lines = params
          .filter(function (p) { return p.value != null && p.value[1] != null; })
          .map(function (p) {
            return p.marker + " " + p.seriesName + "  <b>" + p.value[1] + "</b>";
          });
        return "<div>" + header + "</div>" + lines.join("<br/>");
      };
    </script>
```

- [ ] **Step 3: ChartCanvas learns to attach window functions**

In `echarts.rs`, add a helper and an optional `ChartCanvas` prop:

```rust
/// Graft a JS function defined on `window` (e.g. a tooltip formatter — the
/// JSON option bridge can't carry functions) onto the parsed option object
/// at `target.key`. Silently a no-op when the function is missing, so a
/// stale index.html degrades to ECharts' default formatting.
fn attach_window_fn(option: &JsValue, target: &str, key: &str, window_fn: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(func) = js_sys::Reflect::get(&window, &window_fn.into()) else {
        return;
    };
    if func.is_undefined() {
        return;
    }
    if let Ok(obj) = js_sys::Reflect::get(option, &target.into())
        && !obj.is_undefined()
    {
        let _ = js_sys::Reflect::set(&obj, &key.into(), &func);
    }
}
```

`ChartCanvas` gains `#[prop(optional, into)] tooltip_formatter: Option<&'static str>` — when set, after `let opt = json(...)` and before `set_option_with`, call `attach_window_fn(&opt, "tooltip", "formatter", name)`.

- [ ] **Step 4: Home chart uses it**

```rust
    view! { <crate::echarts::ChartCanvas option reset_zoom=reset tooltip_formatter="chaosTimeTooltip" class="temp-chart"/> }
```

(Weather charts keep their category-axis labels — their tooltip header already carries the emoji/hour label with day at midnight; adopting the helper there is out of scope.)

- [ ] **Step 5: Option-builder test**

Extend home.rs tests: build `chart_option` output (it calls `css_var` — NOT wasm-safe off-browser? Check: `chart_option` calls `crate::echarts::css_var` which panics off-wasm. The existing code has no native tests for chart_option for this reason. So instead assert on a pure fragment: extract the axis formatter into a small `fn time_axis_label_formatter() -> serde_json::Value` and test that it contains the day-level template):

```rust
    #[test]
    fn axis_labels_name_the_day_at_boundaries() {
        let fmt = super::time_axis_label_formatter();
        assert_eq!(fmt["day"], "{ee} {d}");
        assert_eq!(fmt["hour"], "{HH}:{mm}");
    }
```

- [ ] **Step 6: Check, test, build, commit**

`nix develop -c just check && nix develop -c just test`
`nix develop -c sh -c 'cd crates/chaos-web && trunk build'` (index.html change must build).

```bash
git add crates/chaos-web/index.html crates/chaos-ui/src/echarts.rs crates/chaos-ui/src/pages/home.rs
git commit -m "feat(charts): day-aware axis labels and tooltip header on the home chart"
```

---

### Task E: Live verification against production data

- [ ] **Step 1: Serve the branch against zeus**

```bash
nix develop -c sh -c 'cd crates/chaos-web && trunk build --release'
# Serve dist on 60000 with the API pointed at zeus (pattern used in prior sessions):
# a chaos-server instance with static_dir=dist is simplest:
CHAOS_CONFIG=/dev/null nix develop -c sh -c 'cargo run -p chaos-server' # adapt: set listen 0.0.0.0:60000, static_dir crates/chaos-web/dist
```
Simplest concrete recipe: write a scratch TOML (listen = "0.0.0.0:60000", static_dir = "crates/chaos-web/dist") to the scratchpad and run `CHAOS_CONFIG=<that file> cargo run -p chaos-server`, then browse with `window.CHAOS_API_BASE`… — NOTE: the UI resolves its API base from the page origin; to point at zeus use the localStorage override the ServerGate honors, or simply verify weather via the local server (it fetches Open-Meteo directly — Rennes must now work).

- [ ] **Step 2: Verify**
- `curl -s http://localhost:60000/api/v1/widgets/<rennes-widget-id>` returns 200 with weather JSON (fixes the 502).
- Home chart: 7-day data, initial zoom on today, day names visible on the axis when zoomed out, tooltip header shows "Sat 12 Jul, 14:30"-style.

- [ ] **Step 3: Report to the controller** (screenshots optional; curl proof required for the weather fix).
