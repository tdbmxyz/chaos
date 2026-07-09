# Weather View Toggle, Zoom UX, ±16-Day Horizon Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Weather charts span 16 days past + 16 days forecast (default window: past 24 h + next 48 h), get wheel/pinch-zoom + drag-pan + double-click-reset interactions, and a page toggle switches between one-chart-per-location (split) and one multi-line chart (combined).

**Architecture:** The server stops truncating Open-Meteo's hourly series (`past_days=16&forecast_days=16`) and reports `now_index` in `WeatherData`. The shared `ChartCanvas` (echarts.rs) drops the drag-select/step-out machinery; dblclick dispatches a `reset_zoom` window prop, and the same window is dispatched once after the first render (options never carry `dataZoom.start/end`, so reactive re-renders preserve the user's window). The Weather page reverts to one visible series per chart in split mode, deletes the invisible-sibling hack, and adds a combined mode: header-only rows + one chart built by a new `combined_chart_option` from the existing page-wide `loaded` signal (extended to carry `now_index`).

**Tech Stack:** Rust workspace — axum server (`chaos-server`), shared types (`chaos-domain`), Leptos 0.8 CSR/wasm UI (`chaos-ui`), vendored ECharts 5.6.0 via wasm-bindgen (`chaos-ui/src/echarts.rs`), trunk frontend (`chaos-web`).

**Spec:** `docs/superpowers/specs/2026-07-09-weather-view-toggle-horizon-design.md`

**Branch:** `feat/weather-hourly-chart` (continue on it).

**Verification commands:**
- Unit tests: `cargo test -p chaos-ui`, `cargo test -p chaos-server` (plain cargo; nextest NOT installed)
- Full gate: `just check` from repo root (fmt + clippy `-D warnings` + wasm compile)
- IMPORTANT: clippy `-D warnings` fails on unused items. Every task is one self-contained green commit.
- Do not touch the pre-existing dirty files (ci.yml, release.yml, flake.nix, justfile).

**Commit trailer (append to every commit message):**
```
Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_019emSFGLm8F1AFZADq2fQA3
```

---

### Task 1: Server ±16-day horizon + `now_index`

**Files:**
- Modify: `crates/chaos-domain/src/dashboard.rs` (WeatherData, ~line 202)
- Modify: `crates/chaos-server/src/widgets/weather.rs` (fetch URL, truncation removal, tests)

- [ ] **Step 1: Add the domain field**

In `crates/chaos-domain/src/dashboard.rs`, inside `pub struct WeatherData`, replace the `hourly` doc comment and append `now_index` after `hourly`:

```rust
    /// Hour-by-hour series spanning 16 days past through 16 days forecast
    /// (the weather page; the dashboard widget only shows current + daily).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hourly: Vec<HourlyForecast>,
    /// Where "now" sits in `hourly`: index of the first entry at or after
    /// the location-local current hour.
    #[serde(default)]
    pub now_index: usize,
```

- [ ] **Step 2: Find every `WeatherData {` construction site**

Run: `grep -rn "WeatherData {" crates/ --include="*.rs"`
Expected: only `crates/chaos-server/src/widgets/weather.rs` (the fetch function). If others appear (tests, mocks), add `now_index: 0` (or the correct value) to each so the workspace compiles.

- [ ] **Step 3: Write the failing server tests**

Append to `crates/chaos-server/src/widgets/weather.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::now_index;
    use chaos_domain::HourlyForecast;
    use chrono::NaiveDate;

    fn hour(d: u32, h: u32) -> HourlyForecast {
        HourlyForecast {
            time: NaiveDate::from_ymd_opt(2026, 7, d)
                .unwrap()
                .and_hms_opt(h, 0, 0)
                .unwrap(),
            temp_c: 20.0,
            weather_code: 0,
        }
    }

    #[test]
    fn now_index_finds_first_entry_at_or_after_now() {
        let hourly = vec![hour(8, 22), hour(8, 23), hour(9, 0), hour(9, 1)];
        let now = NaiveDate::from_ymd_opt(2026, 7, 9)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        assert_eq!(now_index(&hourly, now), 2);
    }

    #[test]
    fn now_index_matches_exact_hour() {
        let hourly = vec![hour(9, 13), hour(9, 14), hour(9, 15)];
        let now = NaiveDate::from_ymd_opt(2026, 7, 9)
            .unwrap()
            .and_hms_opt(14, 0, 0)
            .unwrap();
        assert_eq!(now_index(&hourly, now), 1);
    }

    #[test]
    fn now_index_is_len_when_everything_is_past() {
        let hourly = vec![hour(1, 10), hour(1, 11)];
        let now = NaiveDate::from_ymd_opt(2026, 7, 9)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        assert_eq!(now_index(&hourly, now), 2);
    }
}
```

- [ ] **Step 4: Run to verify failure**

Run: `cargo test -p chaos-server`
Expected: COMPILE ERROR — `now_index` not found.

- [ ] **Step 5: Implement**

In `crates/chaos-server/src/widgets/weather.rs`:

5a. Change the URL line `&timezone=auto&forecast_days=5",` to:

```rust
         &timezone=auto&forecast_days=16&past_days=16",
```

5b. Replace the truncating hourly build (the comment about "48 points", the `.filter(...)`, and the `.take(48)`) so the whole block reads:

```rust
    let current = forecast.current;
    // The hourly series spans past_days back through the full forecast; the
    // UI needs to know where "now" sits in it, anchored to the local hour.
    let this_hour = {
        use chrono::Timelike;
        current
            .time
            .with_minute(0)
            .and_then(|t| t.with_second(0))
            .unwrap_or(current.time)
    };
    let hourly: Vec<HourlyForecast> = forecast
        .hourly
        .time
        .into_iter()
        .zip(forecast.hourly.temperature_2m)
        .zip(forecast.hourly.weather_code)
        .map(|((time, temp_c), weather_code)| HourlyForecast {
            time,
            temp_c,
            weather_code,
        })
        .collect();
    let now_index = now_index(&hourly, this_hour);
```

5c. Add the pure helper below `fetch` (above `resolve`):

```rust
/// Index of the first hourly entry at or after `this_hour` — where "now"
/// sits in a series that reaches into the past. `hourly.len()` when every
/// entry is in the past (can't happen with a live forecast, but total).
fn now_index(hourly: &[HourlyForecast], this_hour: chrono::NaiveDateTime) -> usize {
    hourly
        .iter()
        .position(|h| h.time >= this_hour)
        .unwrap_or(hourly.len())
}
```

5d. Add `now_index,` to the `WeatherData { ... }` literal (after `hourly,`).

- [ ] **Step 6: Run tests**

Run: `cargo test -p chaos-server && cargo test -p chaos-ui`
Expected: server tests 3 passed; chaos-ui still 15 passed (the UI doesn't construct WeatherData).

- [ ] **Step 7: Full gate**

Run: `just check`
Expected: green.

- [ ] **Step 8: Commit**

```bash
git add crates/chaos-domain/src/dashboard.rs crates/chaos-server/src/widgets/weather.rs
git commit -m "feat(weather): serve 16 days past + 16 days forecast with now_index

The hourly series is no longer truncated to the next 48 h; WeatherData
reports where the location-local current hour sits so the UI can anchor
its default window and now-marker.

<trailer>"
```

(Replace `<trailer>` with the two trailer lines from the header.)

---

### Task 2: ChartCanvas zoom rework (wheel/pan + dblclick reset) + Home option

**Files:**
- Modify: `crates/chaos-ui/src/echarts.rs`
- Modify: `crates/chaos-ui/src/pages/home.rs` (chart_option toolbox/dataZoom block, ~lines 282–299)

No pure logic remains in `echarts.rs` after this task, so no unit tests here — the deleted `widen_window` tests go with it. `cargo test -p chaos-ui` still runs the weather tests.

- [ ] **Step 1: Delete the stepping machinery from `echarts.rs`**

Remove entirely:
- `pub(crate) fn widen_window(...)` (and its doc comment)
- `fn zoom_window(...)` (and its doc comment)
- the `get_option` binding (`/// chart.getOption()...` + `#[wasm_bindgen(method, js_name = getOption)] pub fn get_option(...)`) from the extern block
- the whole `#[cfg(test)] mod tests { ... }` at the bottom (all 6 tests test `widen_window`)

Keep `ZRender`, `get_zr`, and `on` — the dblclick handler still needs them.

- [ ] **Step 2: Rework `ChartCanvas`**

Replace the component with:

```rust
/// A mounted ECharts instance: owns init, option updates, zoom gestures,
/// resize, and disposal. `option` is re-run reactively, so a builder that
/// reads signals re-renders the chart. When `group` is set, the chart joins
/// that ECharts connect-group (shared dataZoom + tooltip across every chart
/// in the group). `reset_zoom` is the default dataZoom window in percent —
/// applied once after the first render and again on every double-click
/// (options never carry `start`/`end`, so reactive re-renders leave a
/// user-adjusted window alone). `class` sizes the container.
#[component]
pub fn ChartCanvas(
    option: Callback<(), serde_json::Value>,
    // `into` lets callers pass `group="weather"` (wrapped to Some) or omit it (None).
    #[prop(optional, into)] group: Option<&'static str>,
    // `into`: pass a bare `(start, end)` tuple or omit for the full range.
    #[prop(optional, into)] reset_zoom: Option<(f64, f64)>,
    class: &'static str,
) -> impl IntoView {
    let node = NodeRef::<leptos::html::Div>::new();
    let chart = StoredValue::new_local(None::<EChart>);
    // The dblclick closure must outlive the JS subscription; it's parked
    // here and dropped on cleanup (after dispose tears down zrender).
    let dblclick = StoredValue::new_local(None::<Closure<dyn FnMut()>>);
    // The default window is dispatched once, after the first set_option of
    // the mount (the dataZoom component must exist before the action).
    let zoomed = StoredValue::new_local(false);
    let failed = RwSignal::new(false);

    Effect::new(move |_| {
        let Some(el) = node.get() else {
            return;
        };
        let instance = match chart.get_value() {
            Some(instance) => instance,
            None => match init(&el) {
                Ok(instance) => {
                    if let Some(group) = group {
                        instance.set_group(group);
                    }
                    // Double-click resets to the default window. In a
                    // connect group the action propagates, so all weather
                    // charts reset together.
                    let reset = {
                        let instance = instance.clone();
                        Closure::wrap(Box::new(move || {
                            zoom_to(&instance, reset_zoom.unwrap_or((0.0, 100.0)));
                        }) as Box<dyn FnMut()>)
                    };
                    {
                        use wasm_bindgen::JsCast;
                        instance
                            .get_zr()
                            .on("dblclick", reset.as_ref().unchecked_ref());
                    }
                    dblclick.set_value(Some(reset));
                    chart.set_value(Some(instance.clone()));
                    instance
                }
                // Bundle missing / init failed: show a message, page still works.
                Err(_) => {
                    failed.set(true);
                    return;
                }
            },
        };
        let opt = json(&option.run(()).to_string());
        // replaceMerge on series only: retracted locations leave the chart
        // (plain merge would keep them in the tooltip forever), while every
        // other component — crucially the dataZoom — merges, preserving the
        // current zoom window as siblings stream in.
        let _ = instance.set_option_with(&opt, &json(r#"{"replaceMerge":["series"]}"#));
        // First render of this mount: apply the default window. Propagating
        // through the connect group is intended — a newly added location
        // realigns every synced chart, keeping the group consistent.
        if !zoomed.get_value() {
            zoomed.set_value(true);
            zoom_to(&instance, reset_zoom.unwrap_or((0.0, 100.0)));
        }
        // (Re)connect the group as members mount asynchronously.
        if let Some(group) = group {
            connect(group);
        }
    });

    let resize = window_event_listener(leptos::ev::resize, move |_| {
        if let Some(instance) = chart.get_value() {
            let _ = instance.resize();
        }
    });
    on_cleanup(move || {
        resize.remove();
        if let Some(instance) = chart.get_value() {
            let _ = instance.dispose();
        }
        dblclick.set_value(None);
    });

    view! {
        <div class=class node_ref=node></div>
        {move || {
            failed
                .get()
                .then(|| view! { <p class="error">"Chart failed to load (echarts bundle missing?)"</p> })
        }}
    }
}
```

Note the `takeGlobalCursor` arming dispatch is GONE (drag now pans via the inside dataZoom; there is no drag-select zoom anymore).

- [ ] **Step 3: Add the `zoom_to` helper**

Where `zoom_window` used to be (above `ChartCanvas`):

```rust
/// Dispatch a dataZoom action setting the window to `[start, end]` percent
/// of the full range.
fn zoom_to(chart: &EChart, (start, end): (f64, f64)) {
    let _ = chart.dispatch_action(&json(&format!(
        r#"{{"type":"dataZoom","start":{start},"end":{end}}}"#
    )));
}
```

- [ ] **Step 4: Update the Home chart option**

In `crates/chaos-ui/src/pages/home.rs`, in `chart_option`, DELETE the toolbox block (the 4-line comment starting `// The toolbox must be *rendered*` plus the `"toolbox": { ... },` entry) and replace the dataZoom comment + block:

```rust
        // Wheel zooms around the cursor, drag pans, touch pinches; wheel
        // never pans (moveOnMouseWheel) so scroll stays predictable.
        "dataZoom": [{
            "type": "inside",
            "xAxisIndex": 0,
            "zoomOnMouseWheel": true,
            "moveOnMouseMove": true,
            "moveOnMouseWheel": false,
        }],
```

`TemperatureChart` (the `ChartCanvas` call site in home.rs) does not pass `reset_zoom` — Home's default and dblclick reset are the full range.

- [ ] **Step 5: Interim check — weather option still references the old world**

`weather.rs` still emits a toolbox block and wheel-disabled dataZoom; that's Task 3's job. It still COMPILES (nothing in weather.rs referenced `widen_window`/`get_option`), so this task stays green. Verify:

Run: `cargo test -p chaos-ui && just check`
Expected: 9 tests pass (15 minus the 6 deleted widen_window tests), gate green. If clippy flags anything unused after the deletions (it shouldn't — `ZRender`/`get_zr`/`on` are used by the handler, `zoom_to` by two call sites), remove the flagged item and re-run.

- [ ] **Step 6: Commit**

```bash
git add crates/chaos-ui/src/echarts.rs crates/chaos-ui/src/pages/home.rs
git commit -m "feat(charts): wheel zoom + drag pan + double-click reset

Drops the drag-select zoom (toolbox + takeGlobalCursor) and the gradual
dblclick step-out. The inside dataZoom now handles wheel-zoom-at-cursor
and drag-pan; dblclick dispatches the new reset_zoom window prop, which
is also applied once after a chart's first render (options never carry
dataZoom start/end, so re-renders preserve a user-adjusted window).

<trailer>"
```

---

### Task 3: Weather split-mode rework (single series, default window, now-marker, labels)

**Files:**
- Modify: `crates/chaos-ui/src/pages/weather.rs`

This task deletes the invisible-sibling mechanism (and `SIBLING_PALETTE` — combined mode re-adds a palette in Task 4), reworks labels for the 32-day scale, adds `default_window`, extends `LoadedForecasts` with `now_index`, and rewires the row chart.

- [ ] **Step 1: Write the failing tests**

In the `#[cfg(test)] mod tests` of `crates/chaos-ui/src/pages/weather.rs`:

1a. DELETE these three tests: `option_embeds_all_locations_one_visible`, `option_prepends_own_row_when_not_yet_in_shared_list`, `sibling_marker_colors_are_stable_by_list_index`.

1b. REPLACE `labels_show_weekday_at_midnight` with:

```rust
    #[test]
    fn labels_show_weekday_and_day_at_midnight() {
        // 2026-07-10 is a Friday; midnight rows carry "Fri 10" so days stay
        // identifiable across a 32-day series.
        let hours = vec![hour(2026, 7, 10, 0, 15.0, 2)];
        assert_eq!(
            hourly_labels(&hours),
            vec![format!("{}\nFri 10", crate::weather_emoji(2))]
        );
    }
```

1c. UPDATE `two_locations` to the three-field `LoadedForecasts` shape (see Step 3) — entries gain a `now_index` of `1`:

```rust
    /// Two loaded locations for option tests: "Osaka" (20–21°C) and
    /// "Nijar" (30–31°C), each with now at index 1.
    fn two_locations() -> LoadedForecasts {
        vec![
            (
                "Osaka".to_string(),
                vec![hour(2026, 7, 9, 14, 20.0, 0), hour(2026, 7, 9, 15, 21.0, 2)],
                1,
            ),
            (
                "Nijar".to_string(),
                vec![hour(2026, 7, 9, 14, 30.0, 1), hour(2026, 7, 9, 15, 31.0, 1)],
                1,
            ),
        ]
    }
```

1d. The two `y_range` tests keep their bodies but their `everyone` construction changes to match (map over 3-tuples):

```rust
    #[test]
    fn y_range_spans_all_locations_padded_outward() {
        let all = two_locations();
        let everyone: Vec<(&str, &[HourlyForecast])> = all
            .iter()
            .map(|(n, h, _)| (n.as_str(), h.as_slice()))
            .collect();
        // min 20, max 31, ±1° padding → (19, 32).
        assert_eq!(y_range(&everyone, false), (19.0, 32.0));
    }

    #[test]
    fn y_range_converts_to_fahrenheit() {
        let all = [("A".to_string(), vec![hour(2026, 7, 9, 14, 20.0, 0)], 0)];
        let everyone: Vec<(&str, &[HourlyForecast])> = all
            .iter()
            .map(|(n, h, _)| (n.as_str(), h.as_slice()))
            .collect();
        // 20°C = 68°F, ±1 → (67, 69).
        assert_eq!(y_range(&everyone, true), (67.0, 69.0));
    }
```

1e. ADD these tests:

```rust
    #[test]
    fn default_window_centers_on_now() {
        // len 100, now at 50 → hours [26, 98] → percent (26, 98).
        assert_eq!(default_window(50, 100), (26.0, 98.0));
    }

    #[test]
    fn default_window_clamps_at_series_start() {
        // now at 10: past-24h reaches before the series → clamp to 0.
        assert_eq!(default_window(10, 100), (0.0, 58.0));
    }

    #[test]
    fn default_window_clamps_at_series_end() {
        // now at 90: next-48h reaches past the series → clamp to 100.
        assert_eq!(default_window(90, 100), (66.0, 100.0));
    }

    #[test]
    fn default_window_full_range_when_empty() {
        assert_eq!(default_window(0, 0), (0.0, 100.0));
    }

    #[test]
    fn split_option_has_one_series_and_now_marker() {
        let all = two_locations();
        let own = all[0].1.clone();
        let opt = weather_chart_option(&own, 1, &all, false, &crate::echarts::ChartColors::default());
        // Exactly one visible series — no sibling embedding, no legend.
        let series = opt["series"].as_array().unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0]["data"], serde_json::json!([20.0, 21.0]));
        assert!(opt["legend"].is_null());
        // Now-marker at the given index.
        assert_eq!(series[0]["markLine"]["data"][0]["xAxis"], 1);
        // y pinned to the page-wide range (both locations): (19, 32).
        assert_eq!(opt["yAxis"]["min"], 19.0);
        assert_eq!(opt["yAxis"]["max"], 32.0);
        // Wheel zoom + drag pan on; no start/end in the option (the reset
        // window is dispatched by ChartCanvas, not merged on re-renders).
        assert_eq!(opt["dataZoom"][0]["zoomOnMouseWheel"], true);
        assert_eq!(opt["dataZoom"][0]["moveOnMouseMove"], true);
        assert!(opt["dataZoom"][0]["start"].is_null());
        assert!(opt["dataZoom"][0]["end"].is_null());
        // No toolbox (drag-select zoom is gone); labels auto-thin.
        assert!(opt["toolbox"].is_null());
        assert!(opt["xAxis"]["axisLabel"]["interval"].is_null());
        assert_eq!(opt["xAxis"]["axisLabel"]["hideOverlap"], true);
    }

    #[test]
    fn split_option_y_range_includes_own_unpublished_data() {
        // Own data not in the shared list still shapes the y-range.
        let all = two_locations();
        let own = vec![hour(2026, 7, 9, 14, 10.0, 0)];
        let opt = weather_chart_option(&own, 0, &all, false, &crate::echarts::ChartColors::default());
        assert_eq!(opt["yAxis"]["min"], 9.0);
        assert_eq!(opt["yAxis"]["max"], 32.0);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p chaos-ui`
Expected: COMPILE ERROR — `default_window` not found, `weather_chart_option` signature mismatch, tuple arity mismatch.

- [ ] **Step 3: Implement in `weather.rs`**

3a. Extend the alias (replace the existing one):

```rust
/// Every loaded location's hourly forecast plus its `now_index`, insertion-
/// ordered as fetches resolve; keyed by the API's resolved display name.
/// Charts read it for the shared y-range and the combined view.
type LoadedForecasts = Vec<(String, Vec<chaos_domain::HourlyForecast>, usize)>;
```

3b. `hourly_labels`: change the midnight branch from `h.time.format("%a")` to:

```rust
            let below = if h.time.hour() == 0 {
                // "Fri 10" — day boundaries stay identifiable across weeks.
                h.time.format("%a %-d").to_string()
            } else {
                format!("{}h", h.time.hour())
            };
```

(Update the function doc comment: `the weekday and day-of-month ("Fri 10") at midnight`.)

3c. DELETE `SIBLING_PALETTE` (Task 4 re-adds a palette for the combined view).

3d. Add `default_window` below `y_range`:

```rust
/// The default visible window — past 24 h through next 48 h — as dataZoom
/// percentages of the full series, clamped to [0, 100]. Full range for an
/// empty series.
fn default_window(now_index: usize, len: usize) -> (f64, f64) {
    if len == 0 {
        return (0.0, 100.0);
    }
    let len = len as f64;
    let start = (now_index as f64 - 24.0).max(0.0) / len * 100.0;
    let end = (now_index as f64 + 48.0).min(len) / len * 100.0;
    (start, end)
}
```

3e. Replace `weather_chart_option` entirely:

```rust
/// The ECharts option for one location in split view. Category x-axis (one
/// slot per hour) so each tick carries an emoji-over-hour label and cross-
/// location zoom aligns by forecast hour, not wall-clock. One visible
/// series; the y-axis is pinned to the page-wide range (every loaded
/// location, plus this row's own data in case its publish hasn't landed —
/// duplicates can't move a min/max) so charts compare at a glance. A dashed
/// mark line separates past from forecast. Colours are injected by the
/// caller so this stays pure/testable.
fn weather_chart_option(
    hourly: &[chaos_domain::HourlyForecast],
    now_index: usize,
    all: &LoadedForecasts,
    fahrenheit: bool,
    colors: &crate::echarts::ChartColors,
) -> serde_json::Value {
    let unit = if fahrenheit { "°F" } else { "°C" };
    let labels = hourly_labels(hourly);

    let text = colors.text.as_str();
    let muted = colors.muted.as_str();
    let border = colors.border.as_str();
    let surface = colors.surface.as_str();
    let accent = colors.accent.as_str();

    let mut everyone: Vec<(&str, &[chaos_domain::HourlyForecast])> = all
        .iter()
        .map(|(name, hourly, _)| (name.as_str(), hourly.as_slice()))
        .collect();
    everyone.push(("", hourly));
    let (y_min, y_max) = y_range(&everyone, fahrenheit);

    serde_json::json!({
        "animation": false,
        "grid": { "left": 44, "right": 16, "top": 20, "bottom": 40 },
        // No `valueFormatter` here: ECharts wants a JS function for it and the
        // JSON bridge can't carry one, so the axis tooltip shows the raw
        // one-decimal number — the y-axis already labels the unit.
        "tooltip": {
            "trigger": "axis",
            "backgroundColor": surface,
            "borderColor": border,
            "textStyle": { "color": text },
        },
        // Wheel zooms around the cursor, drag pans, touch pinches. No
        // start/end here: ChartCanvas dispatches the default window once,
        // so reactive re-renders don't snap a user-adjusted window back.
        "dataZoom": [{
            "type": "inside",
            "xAxisIndex": 0,
            "zoomOnMouseWheel": true,
            "moveOnMouseMove": true,
            "moveOnMouseWheel": false,
        }],
        "xAxis": {
            "type": "category",
            "data": labels,
            // Auto-thinned labels: density adapts to the zoom level across
            // the 32-day series.
            "axisLabel": { "color": muted, "hideOverlap": true, "lineHeight": 16 },
            "axisLine": { "lineStyle": { "color": border } },
            "axisTick": { "alignWithLabel": true },
        },
        "yAxis": {
            "type": "value",
            "min": y_min,
            "max": y_max,
            "axisLabel": { "color": muted, "formatter": format!("{{value}}{unit}") },
            "splitLine": { "lineStyle": { "color": border } },
        },
        "series": [{
            "name": "Temperature",
            "type": "line",
            "showSymbol": false,
            "color": accent,
            "lineStyle": { "width": 1.5 },
            "data": hourly_temps(hourly, fahrenheit),
            "markLine": {
                "silent": true,
                "symbol": "none",
                "label": { "show": true, "formatter": "now", "color": muted },
                "lineStyle": { "color": muted, "type": "dashed", "width": 1 },
                "data": [{ "xAxis": now_index }],
            },
        }],
    })
}
```

3f. In the publish `Effect` of `WeatherRow`, carry `now_index` (the destructuring line and both upsert arms change):

```rust
        let (name, hourly, now_index) = (weather.location, weather.hourly, weather.now_index);
        published.set_value(Some(name.clone()));
        // Known edge: two place strings resolving to the same display name
        // share one entry, and removing one row retracts the entry the other
        // still uses — self-healing, because the y-range folds each row's
        // own data in regardless.
        loaded.update(|list| match list.iter_mut().find(|(n, _, _)| *n == name) {
            Some(entry) => (entry.1, entry.2) = (hourly.clone(), now_index),
            None => list.push((name.clone(), hourly.clone(), now_index)),
        });
```

And the `on_cleanup` retain becomes `list.retain(|(n, _, _)| n != &name)`.

3g. In `weather_row_body`, the chart block becomes (the `let location = weather.location.clone();` first line is now unused by the chart — remove it ONLY if nothing else uses `location`; the head view uses `weather.location` directly, so remove the clone):

```rust
        {
            let hourly = weather.hourly;
            let now_index = weather.now_index;
            if hourly.is_empty() {
                // Mirror the Home chart's empty state rather than showing an
                // axes-only, line-less box.
                view! { <p class="muted">"No hourly forecast."</p> }.into_any()
            } else {
                let colors = crate::echarts::ChartColors::from_theme();
                let reset = default_window(now_index, hourly.len());
                let option = Callback::new(move |()| {
                    let all = loaded.get();
                    weather_chart_option(&hourly, now_index, &all, fahrenheit, &colors)
                });
                view! {
                    <crate::echarts::ChartCanvas
                        option
                        group="weather"
                        reset_zoom=reset
                        class="weather-chart"
                    />
                }
                .into_any()
            }
        }
```

3h. Update the stale doc comments: the `WeatherRow` doc (`hourly strip (48 h, scrolls sideways...)`) becomes `/// One location: current conditions plus the ±16-day hourly chart.`, and the `y_range` doc's "Precondition" note stays as-is.

- [ ] **Step 4: Run tests**

Run: `cargo test -p chaos-ui`
Expected: PASS — 12 tests (3 label/temp tests with the midnight one updated, 2 y_range, 4 default_window, 2 split_option, +1 temps... count them: `labels_show_emoji_over_hour`, `labels_show_weekday_and_day_at_midnight`, `temps_convert_and_round_celsius`, `temps_convert_and_round_fahrenheit`, `y_range_spans_all_locations_padded_outward`, `y_range_converts_to_fahrenheit`, 4× `default_window_*`, `split_option_has_one_series_and_now_marker`, `split_option_y_range_includes_own_unpublished_data` = 12).

- [ ] **Step 5: Full gate**

Run: `just check`
Expected: green (SIBLING_PALETTE deleted, so no dead code).

- [ ] **Step 6: Commit**

```bash
git add crates/chaos-ui/src/pages/weather.rs
git commit -m "feat(weather): split view reverts to one series over the +-16-day range

Drops the invisible-sibling tooltip hack. Each chart draws its own line,
opens on past-24h..next-48h (default_window, dispatched via reset_zoom),
marks now with a dashed line, auto-thins labels across the 32-day series
(midnight ticks carry 'Fri 10'), and keeps the page-wide pinned y-range.
LoadedForecasts entries carry now_index for the combined view to come.

<trailer>"
```

---

### Task 4: Combined view toggle

**Files:**
- Modify: `crates/chaos-ui/src/pages/weather.rs`
- Modify: `crates/chaos-web/styles.css`

- [ ] **Step 1: Write the failing tests**

Add to the tests module in `weather.rs`:

```rust
    #[test]
    fn combined_option_has_one_visible_series_per_location() {
        let all = two_locations();
        let opt = combined_chart_option(&all, false, &crate::echarts::ChartColors::default());
        let series = opt["series"].as_array().unwrap();
        assert_eq!(series.len(), 2);
        // Both visible, palette-colored, named after their location.
        assert_eq!(series[0]["name"], "Osaka");
        assert_eq!(series[1]["name"], "Nijar");
        for s in series {
            assert!(s["lineStyle"]["opacity"].is_null());
            assert!(s["color"].as_str().unwrap().starts_with('#'));
        }
        assert_ne!(series[0]["color"], series[1]["color"]);
        // Legend names the locations; tooltip compares them natively.
        assert_eq!(opt["legend"]["data"], serde_json::json!(["Osaka", "Nijar"]));
        // Now-marker rides the first series, at the first entry's now_index.
        assert_eq!(series[0]["markLine"]["data"][0]["xAxis"], 1);
        assert!(series[1]["markLine"].is_null());
        // Same pinned page-wide y-range as split view.
        assert_eq!(opt["yAxis"]["min"], 19.0);
        assert_eq!(opt["yAxis"]["max"], 32.0);
        // Labels come from the FIRST location, without the emoji line.
        assert_eq!(opt["xAxis"]["data"], serde_json::json!(["14h", "15h"]));
        assert_eq!(opt["xAxis"]["axisLabel"]["hideOverlap"], true);
    }

    #[test]
    fn combined_labels_have_no_emoji() {
        // Midnight: "Fri 10"; other hours: "14h" — same rhythm as split
        // labels minus the per-location emoji line.
        let hours = vec![hour(2026, 7, 10, 0, 15.0, 2), hour(2026, 7, 10, 14, 20.0, 0)];
        assert_eq!(time_labels(&hours), vec!["Fri 10", "14h"]);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p chaos-ui`
Expected: COMPILE ERROR — `combined_chart_option`, `time_labels` not found.

- [ ] **Step 3: Implement the combined option builder**

3a. Add below `hourly_labels`:

```rust
/// Single-line time labels for the combined chart: `"14h"`, or `"Fri 10"`
/// at midnight — the split view's rhythm minus the emoji line (emoji are
/// per-location, and this chart shows every location).
fn time_labels(hourly: &[chaos_domain::HourlyForecast]) -> Vec<String> {
    hourly
        .iter()
        .map(|h| {
            if h.time.hour() == 0 {
                h.time.format("%a %-d").to_string()
            } else {
                format!("{}h", h.time.hour())
            }
        })
        .collect()
}
```

3b. Add the palette (below `time_labels`):

```rust
/// Line colors for the combined chart, one per location by list index.
const LOCATION_PALETTE: [&str; 6] = [
    "#5470c6", "#91cc75", "#fac858", "#ee6666", "#73c0de", "#9a60b4",
];
```

3c. Add the builder below `weather_chart_option`:

```rust
/// The combined view: every loaded location as a visible line in one chart,
/// with a legend and the native multi-series axis tooltip doing the
/// comparison. Same pinned y-range, zoom gestures, and now-marker (on the
/// first series, from the first location's clock) as the split charts; the
/// x-axis borrows the first location's timestamps, so cross-timezone rows
/// pair by hour index, not wall-clock. Precondition: `all` is non-empty
/// (the caller renders an empty state instead).
fn combined_chart_option(
    all: &LoadedForecasts,
    fahrenheit: bool,
    colors: &crate::echarts::ChartColors,
) -> serde_json::Value {
    let unit = if fahrenheit { "°F" } else { "°C" };
    let (_, first_hourly, first_now) = &all[0];
    let labels = time_labels(first_hourly);

    let text = colors.text.as_str();
    let muted = colors.muted.as_str();
    let border = colors.border.as_str();
    let surface = colors.surface.as_str();

    let everyone: Vec<(&str, &[chaos_domain::HourlyForecast])> = all
        .iter()
        .map(|(name, hourly, _)| (name.as_str(), hourly.as_slice()))
        .collect();
    let (y_min, y_max) = y_range(&everyone, fahrenheit);

    let names: Vec<&str> = all.iter().map(|(name, _, _)| name.as_str()).collect();
    let series: Vec<serde_json::Value> = all
        .iter()
        .enumerate()
        .map(|(i, (name, hourly, _))| {
            let mut s = serde_json::json!({
                "name": name,
                "type": "line",
                "showSymbol": false,
                "color": LOCATION_PALETTE[i % LOCATION_PALETTE.len()],
                "lineStyle": { "width": 1.5 },
                "data": hourly_temps(hourly, fahrenheit),
            });
            if i == 0 {
                s["markLine"] = serde_json::json!({
                    "silent": true,
                    "symbol": "none",
                    "label": { "show": true, "formatter": "now", "color": muted },
                    "lineStyle": { "color": muted, "type": "dashed", "width": 1 },
                    "data": [{ "xAxis": first_now }],
                });
            }
            s
        })
        .collect();

    serde_json::json!({
        "animation": false,
        "grid": { "left": 44, "right": 16, "top": 36, "bottom": 40 },
        "legend": { "top": 0, "data": names, "textStyle": { "color": text }, "inactiveColor": muted },
        "tooltip": {
            "trigger": "axis",
            "backgroundColor": surface,
            "borderColor": border,
            "textStyle": { "color": text },
        },
        // Same gestures as the split charts; no start/end (see split view).
        "dataZoom": [{
            "type": "inside",
            "xAxisIndex": 0,
            "zoomOnMouseWheel": true,
            "moveOnMouseMove": true,
            "moveOnMouseWheel": false,
        }],
        "xAxis": {
            "type": "category",
            "data": labels,
            "axisLabel": { "color": muted, "hideOverlap": true },
            "axisLine": { "lineStyle": { "color": border } },
            "axisTick": { "alignWithLabel": true },
        },
        "yAxis": {
            "type": "value",
            "min": y_min,
            "max": y_max,
            "axisLabel": { "color": muted, "formatter": format!("{{value}}{unit}") },
            "splitLine": { "lineStyle": { "color": border } },
        },
        "series": series,
    })
}
```

- [ ] **Step 4: Run the new tests**

Run: `cargo test -p chaos-ui`
Expected: the 2 new tests pass (14 total). Everything compiles but `combined_chart_option` has no non-test caller yet — clippy WILL flag dead code, so do NOT run `just check` yet; Steps 5–7 wire it up within this same commit.

- [ ] **Step 5: The toggle in `WeatherPage`**

5a. Add the mode signal after `loaded`:

```rust
    let combined = RwSignal::new(false);
```

5b. In the `weather-page-head` view, after the `</form>`:

```rust
                <button
                    class="view-toggle"
                    title="Switch between one chart per place and one combined chart"
                    on:click=move |_| combined.update(|c| *c = !*c)
                >
                    {move || if combined.get() { "Split" } else { "Combine" }}
                </button>
```

5c. Rows must stay mounted across toggles (their fetches would otherwise re-run), so `combined` is passed INTO the rows (charts hide reactively inside) and the combined chart is appended after them. Replace the list closure and add the combined section — the whole `{move || { let list = places.get(); ... }}` block plus a new sibling:

```rust
            {move || {
                let list = places.get();
                if list.is_empty() {
                    // Same place the dashboard widget shows.
                    view! { <WeatherRow location=None on_remove=None loaded combined/> }.into_any()
                } else {
                    list.into_iter()
                        .map(|place| {
                            view! {
                                <WeatherRow
                                    location=Some(place)
                                    on_remove=Some(remove)
                                    loaded
                                    combined
                                />
                            }
                        })
                        .collect_view()
                        .into_any()
                }
            }}
            <Show when=move || combined.get()>
                <CombinedChart loaded/>
            </Show>
```

- [ ] **Step 6: Thread `combined` through the row and hide the split chart**

6a. `WeatherRow` gains the prop:

```rust
#[component]
fn WeatherRow(
    location: Option<String>,
    on_remove: Option<Callback<String>>,
    loaded: RwSignal<LoadedForecasts>,
    combined: RwSignal<bool>,
) -> impl IntoView {
```

6b. Its body call becomes `weather_row_body(weather, loaded, combined).into_any()`, and `weather_row_body` gains the parameter:

```rust
fn weather_row_body(
    weather: WeatherData,
    loaded: RwSignal<LoadedForecasts>,
    combined: RwSignal<bool>,
) -> impl IntoView {
```

6c. In `weather_row_body`'s chart block, wrap the `ChartCanvas` in a `Show` so combined mode renders header-only rows (the chart unmounts/remounts on toggle — cheap, and the row's fetch is untouched). `Callback` is `Copy`, so it moves into the `Show` children freely:

```rust
                view! {
                    <Show when=move || !combined.get()>
                        <crate::echarts::ChartCanvas
                            option
                            group="weather"
                            reset_zoom=reset
                            class="weather-chart"
                        />
                    </Show>
                }
                .into_any()
```

- [ ] **Step 7: The `CombinedChart` component**

Add after `weather_row_body`:

```rust
/// One chart, every location. Remounts only when the FIRST loaded entry's
/// shape changes (a Memo keys it), so later siblings streaming in update
/// the mounted chart via set_option instead of resetting it; the option
/// callback reads `loaded` so those updates are reactive.
#[component]
fn CombinedChart(loaded: RwSignal<LoadedForecasts>) -> impl IntoView {
    let fahrenheit = crate::weather_fahrenheit();
    let first = Memo::new(move |_| {
        loaded.with(|list| list.first().map(|(_, hourly, now)| (hourly.len(), *now)))
    });
    view! {
        <section class="weather-row">
            {move || match first.get() {
                None => view! { <p class="muted">"Loading forecast…"</p> }.into_any(),
                Some((len, now_index)) => {
                    let colors = crate::echarts::ChartColors::from_theme();
                    let reset = default_window(now_index, len);
                    let option = Callback::new(move |()| {
                        let all = loaded.get();
                        combined_chart_option(&all, fahrenheit, &colors)
                    });
                    view! {
                        <crate::echarts::ChartCanvas
                            option
                            reset_zoom=reset
                            class="combined-chart"
                        />
                    }
                    .into_any()
                }
            }}
        </section>
    }
}
```

(No `group`: the combined chart is alone; split charts are unmounted in this mode.)

- [ ] **Step 8: CSS**

In `crates/chaos-web/styles.css`, next to the existing `.weather-chart` rules (~line 1740):

```css
.combined-chart {
  height: 320px;
  margin-top: 0.25rem;
}

.view-toggle {
  white-space: nowrap;
}
```

And inside the existing `@media (max-width: 40rem)` block that shrinks `.weather-chart`:

```css
  .combined-chart {
    height: 240px;
  }
```

- [ ] **Step 9: Run tests and the full gate**

Run: `cargo test -p chaos-ui && just check`
Expected: 14 tests pass; gate green (combined_chart_option, time_labels, LOCATION_PALETTE all reachable from CombinedChart).

- [ ] **Step 10: Commit**

```bash
git add crates/chaos-ui/src/pages/weather.rs crates/chaos-web/styles.css
git commit -m "feat(weather): split/combined view toggle

A header button switches the page between one chart per location and one
combined multi-line chart (legend + native tooltip for comparison, same
pinned y-range, gestures, and now-marker). Rows stay mounted across
toggles so forecasts never refetch; in combined mode they render their
current-conditions header only.

<trailer>"
```

---

### Task 5: Manual smoke test (no code)

Serve for a remote browser:

```bash
# Terminal 1
just server
# Terminal 2
cd crates/chaos-web && trunk serve --address 0.0.0.0 --port 60000
```

At `http://<host>:60000/`, Weather tab with ≥2 locations:

- [ ] Charts open on ~3 days (past 24 h + next 48 h) with a dashed "now" line; panning left reveals ~2 weeks of history, right the full forecast.
- [ ] Wheel over a chart zooms in AND out around the cursor; drag pans; on phone (or devtools touch emulation) pinch works both ways; double-click snaps back to the default window. In split mode all location charts move together.
- [ ] Labels stay readable at every zoom level (auto-thinning); midnight ticks read "Fri 10"-style.
- [ ] "Combine" swaps to compact headers + one chart with a legend, one colored line per location, tooltip comparing all; "Split" swaps back. Toggling repeatedly does NOT refetch (no loading flickers; check the network tab).
- [ ] Removing a location updates the combined chart and the split y-ranges; adding one realigns synced charts to the default window (expected).
- [ ] Home tab: wheel-zoom/pan works on the temperature chart; double-click resets to the full range.
