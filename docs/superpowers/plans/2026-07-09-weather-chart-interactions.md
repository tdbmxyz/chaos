# Weather Chart Interactions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Double-click steps chart zoom back out gradually (Home + Weather); hovering a weather chart shows every location's temperature in one tooltip; all weather charts share one fixed y-range.

**Architecture:** Zoom-out lives in the shared `ChartCanvas` component (`echarts.rs`): a zrender `dblclick` handler reads the live dataZoom window via `getOption()`, widens it 2× with a pure `widen_window` function, and dispatches a `dataZoom` action (the "weather" connect group propagates it to every weather chart; Home has no group and steps out alone). Tooltip + y-scale: every weather chart's option embeds **all** loaded locations as series — its own accent-colored and visible, the others with `lineStyle.opacity: 0` but a stable palette color — so ECharts' native axis tooltip lists them all with no JS formatter (the JSON option bridge can't carry functions). A page-level `RwSignal<Vec<(String, Vec<HourlyForecast>)>>` collects each row's forecast as it loads; option builders read it reactively. The y-axis gets explicit `min`/`max` computed in Rust from the global min/max (±1°, rounded outward), so it's identical everywhere and fixed under zoom.

**Tech Stack:** Rust, Leptos 0.8 (CSR/wasm), vendored Apache ECharts 5.6.0 via hand-written wasm-bindgen bindings, serde_json options.

**Spec:** `docs/superpowers/specs/2026-07-09-weather-chart-interactions-design.md`

**Branch:** `feat/weather-hourly-chart` (continue on it — this extends the feature already on the branch).

**Verification commands:**
- Unit tests: `cargo test -p chaos-ui` (plain cargo; nextest is NOT installed)
- Full gate: `just check` (fmt + clippy `-D warnings` + wasm compile) — run from repo root
- IMPORTANT: clippy runs with `-D warnings`; any item added but not yet used fails the build. Every task below is a self-contained green commit — do not split tasks into smaller commits.

**Commit trailer (append to every commit message):**
```
Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_019emSFGLm8F1AFZADq2fQA3
```

---

### Task 1: Double-click zoom-out in `ChartCanvas`

**Files:**
- Modify: `crates/chaos-ui/src/echarts.rs` (bindings + `widen_window` + handler; tests appended in a `#[cfg(test)]` module at the bottom of the same file)

- [ ] **Step 1: Write the failing tests**

Append to `crates/chaos-ui/src/echarts.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::widen_window;

    #[test]
    fn widens_twofold_around_center() {
        assert_eq!(widen_window(40.0, 60.0), (30.0, 70.0));
    }

    #[test]
    fn clamps_at_left_edge() {
        // [0, 10] doubles to a 20-wide window; can't go below 0, so it
        // grows rightward.
        assert_eq!(widen_window(0.0, 10.0), (0.0, 20.0));
    }

    #[test]
    fn clamps_at_right_edge() {
        assert_eq!(widen_window(90.0, 100.0), (80.0, 100.0));
    }

    #[test]
    fn full_range_is_a_fixed_point() {
        assert_eq!(widen_window(0.0, 100.0), (0.0, 100.0));
    }

    #[test]
    fn degenerate_window_gets_minimum_span() {
        // A zero-width window (fully zoomed) opens to a 5% span.
        assert_eq!(widen_window(50.0, 50.0), (47.5, 52.5));
    }

    #[test]
    fn near_full_span_caps_at_full_range() {
        // 2 × 60 caps at 100, centered on 50 → the full range.
        assert_eq!(widen_window(20.0, 80.0), (0.0, 100.0));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run (from repo root): `cargo test -p chaos-ui`
Expected: COMPILE ERROR — `widen_window` not found.

- [ ] **Step 3: Implement `widen_window`**

Add above the `ChartCanvas` component in `crates/chaos-ui/src/echarts.rs`:

```rust
/// Widen a dataZoom window `[start, end]` (percentages of the full range) by
/// 2× around its center, clamped to `[0, 100]` — one gradual "zoom out" step.
/// Repeated calls walk any window back to the full range; a degenerate
/// (zero-width) window opens to a 5% span so it can't get stuck.
pub(crate) fn widen_window(start: f64, end: f64) -> (f64, f64) {
    let span = ((end - start) * 2.0).max(5.0).min(100.0);
    let center = (start + end) / 2.0;
    let mut s = center - span / 2.0;
    let mut e = center + span / 2.0;
    if s < 0.0 {
        e = (e - s).min(100.0);
        s = 0.0;
    }
    if e > 100.0 {
        s = (s - (e - 100.0)).max(0.0);
        e = 100.0;
    }
    (s, e)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p chaos-ui`
Expected: PASS — 6 new tests plus the 5 existing weather tests, 11 total, 0 failed.

- [ ] **Step 5: Add the ECharts bindings**

Inside the existing `#[wasm_bindgen] extern "C" { ... }` block in `crates/chaos-ui/src/echarts.rs` (after the `connect` binding), add:

```rust
    /// The chart's underlying zrender handle — used for raw canvas events
    /// (`dblclick`) that the chart instance doesn't re-emit for blank areas.
    pub type ZRender;

    #[wasm_bindgen(method, js_name = getZr)]
    pub fn get_zr(this: &EChart) -> ZRender;

    /// `zr.on(event, handler)` — subscribe to a raw canvas event.
    #[wasm_bindgen(method)]
    pub fn on(this: &ZRender, event: &str, handler: &js_sys::Function);

    /// `chart.getOption()` — the live, normalized option; read to learn the
    /// current dataZoom window.
    #[wasm_bindgen(method, js_name = getOption)]
    pub fn get_option(this: &EChart) -> JsValue;
```

- [ ] **Step 6: Add the window reader**

Add next to `widen_window` (this touches wasm-bindgen types, so it stays out of the unit tests — the pure math is what's tested):

```rust
/// The chart's current dataZoom window in percent, `(0, 100)` when the
/// option carries no dataZoom (never the case for our charts, but the
/// fallback keeps the handler total).
fn zoom_window(chart: &EChart) -> (f64, f64) {
    use wasm_bindgen::JsCast;
    let opt = chart.get_option();
    let first = js_sys::Reflect::get(&opt, &"dataZoom".into())
        .ok()
        .and_then(|v| v.dyn_into::<js_sys::Array>().ok())
        .and_then(|arr| (arr.length() > 0).then(|| arr.get(0)));
    let field = |obj: &JsValue, name: &str, default: f64| {
        js_sys::Reflect::get(obj, &name.into())
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(default)
    };
    match first {
        Some(dz) => (field(&dz, "start", 0.0), field(&dz, "end", 100.0)),
        None => (0.0, 100.0),
    }
}
```

- [ ] **Step 7: Wire the dblclick handler into `ChartCanvas`**

In the `ChartCanvas` component:

7a. Below `let chart = StoredValue::new_local(None::<EChart>);` add:

```rust
    // The dblclick closure must outlive the JS subscription; it's parked
    // here and dropped on cleanup (after dispose tears down zrender).
    let dblclick = StoredValue::new_local(None::<Closure<dyn FnMut()>>);
```

(`Closure` is already in scope via `use wasm_bindgen::prelude::*;` at the top of the file.)

7b. In the `Effect`, replace the `Ok(instance) => { ... }` init arm:

```rust
                Ok(instance) => {
                    if let Some(group) = group {
                        instance.set_group(group);
                    }
                    // Double-click steps the zoom back out: widen the current
                    // window 2× and dispatch it. In a connect group the action
                    // propagates, so all weather charts step out together.
                    let zoom_out = {
                        let instance = instance.clone();
                        Closure::wrap(Box::new(move || {
                            let (start, end) = zoom_window(&instance);
                            let (start, end) = widen_window(start, end);
                            let _ = instance.dispatch_action(&json(&format!(
                                r#"{{"type":"dataZoom","start":{start},"end":{end}}}"#
                            )));
                        }) as Box<dyn FnMut()>)
                    };
                    {
                        use wasm_bindgen::JsCast;
                        instance.get_zr().on("dblclick", zoom_out.as_ref().unchecked_ref());
                    }
                    dblclick.set_value(Some(zoom_out));
                    chart.set_value(Some(instance.clone()));
                    instance
                }
```

7c. In the `on_cleanup` closure, after the `dispose()` call, add:

```rust
        dblclick.set_value(None);
```

- [ ] **Step 8: Run the full gate**

Run: `cargo test -p chaos-ui && just check`
Expected: tests PASS; fmt/clippy/wasm all green. If clippy complains about `.max(5.0).min(100.0)` (`clippy::manual_clamp`), rewrite as `.clamp(5.0, 100.0)` — but only if it actually fires.

- [ ] **Step 9: Commit**

```bash
git add crates/chaos-ui/src/echarts.rs
git commit -m "feat(charts): double-click steps zoom back out gradually

A zrender dblclick handler on every ChartCanvas widens the dataZoom window
2x around its center (pure widen_window, unit-tested) and dispatches it;
the weather connect group propagates the action so all location charts
step out together. Pinch-out on touch already works via the inside
dataZoom.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_019emSFGLm8F1AFZADq2fQA3"
```

---

### Task 2: All-locations series + shared fixed y-range in the weather option

**Files:**
- Modify: `crates/chaos-ui/src/pages/weather.rs` (option builder, its call site, tests)

Context: `weather_chart_option` currently takes `(hourly, fahrenheit, colors)` and builds one series with `yAxis.scale: true`. It grows to take the row's identity plus every loaded location, emit one series per location (only the own one visible), and pin the y-axis to a page-wide range. The call site passes an empty "all" list in this task (single-location behavior, now with a fixed y-range); Task 3 feeds it the real shared list.

- [ ] **Step 1: Write the failing tests**

In `crates/chaos-ui/src/pages/weather.rs`, replace the existing `option_has_category_axis_and_one_series` test with the following tests (keep the other four tests and the `hour` helper as they are):

```rust
    /// Two loaded locations for option tests: "Osaka" (20–21°C) and
    /// "Nijar" (30–31°C).
    fn two_locations() -> Vec<(String, Vec<HourlyForecast>)> {
        vec![
            (
                "Osaka".to_string(),
                vec![hour(2026, 7, 9, 14, 20.0, 0), hour(2026, 7, 9, 15, 21.0, 2)],
            ),
            (
                "Nijar".to_string(),
                vec![hour(2026, 7, 9, 14, 30.0, 1), hour(2026, 7, 9, 15, 31.0, 1)],
            ),
        ]
    }

    #[test]
    fn y_range_spans_all_locations_padded_outward() {
        let all = two_locations();
        let everyone: Vec<(&str, &[HourlyForecast])> = all
            .iter()
            .map(|(n, h)| (n.as_str(), h.as_slice()))
            .collect();
        // min 20, max 31, ±1° padding → (19, 32).
        assert_eq!(y_range(&everyone, false), (19.0, 32.0));
    }

    #[test]
    fn y_range_converts_to_fahrenheit() {
        let all = vec![("A".to_string(), vec![hour(2026, 7, 9, 14, 20.0, 0)])];
        let everyone: Vec<(&str, &[HourlyForecast])> = all
            .iter()
            .map(|(n, h)| (n.as_str(), h.as_slice()))
            .collect();
        // 20°C = 68°F, ±1 → (67, 69).
        assert_eq!(y_range(&everyone, true), (67.0, 69.0));
    }

    #[test]
    fn option_embeds_all_locations_one_visible() {
        let all = two_locations();
        let own_hourly = all[0].1.clone();
        let opt = weather_chart_option(
            "Osaka",
            &own_hourly,
            &all,
            false,
            &crate::echarts::ChartColors::default(),
        );
        let series = opt["series"].as_array().unwrap();
        assert_eq!(series.len(), 2);
        // Own series: named, visible (no opacity-0), not silent.
        assert_eq!(series[0]["name"], "Osaka");
        assert!(series[0]["lineStyle"]["opacity"].is_null());
        assert!(series[0]["silent"].is_null());
        assert_eq!(series[0]["data"], serde_json::json!([20.0, 21.0]));
        // Sibling: named, hidden line, silent, but a real marker color.
        assert_eq!(series[1]["name"], "Nijar");
        assert_eq!(series[1]["lineStyle"]["opacity"], 0);
        assert_eq!(series[1]["silent"], true);
        assert!(series[1]["color"].as_str().unwrap().starts_with('#'));
        // Shared fixed y-range replaces scale:true.
        assert_eq!(opt["yAxis"]["min"], 19.0);
        assert_eq!(opt["yAxis"]["max"], 32.0);
        assert!(opt["yAxis"]["scale"].is_null());
        // x-axis still carries the OWN location's emoji labels.
        assert_eq!(opt["xAxis"]["type"], "category");
        assert_eq!(opt["xAxis"]["axisLabel"]["interval"], 2);
        assert_eq!(
            opt["xAxis"]["data"],
            serde_json::json!([
                format!("{}\n14h", crate::weather_emoji(0)),
                format!("{}\n15h", crate::weather_emoji(2)),
            ])
        );
    }

    #[test]
    fn option_prepends_own_row_when_not_yet_in_shared_list() {
        // The row renders before its Effect publishes into the shared list;
        // the builder folds the own data in so the chart never renders empty.
        let all = two_locations();
        let own = vec![hour(2026, 7, 9, 14, 10.0, 0)];
        let opt = weather_chart_option(
            "Palma",
            &own,
            &all,
            false,
            &crate::echarts::ChartColors::default(),
        );
        let series = opt["series"].as_array().unwrap();
        assert_eq!(series.len(), 3);
        assert_eq!(series[0]["name"], "Palma");
        assert!(series[0]["lineStyle"]["opacity"].is_null());
        // Own data participates in the y-range: min 10, max 31 → (9, 32).
        assert_eq!(opt["yAxis"]["min"], 9.0);
        assert_eq!(opt["yAxis"]["max"], 32.0);
    }

    #[test]
    fn sibling_marker_colors_are_stable_by_list_index() {
        // A location's tooltip marker color comes from its index in the
        // shared list, so it's identical on every chart of the page.
        let all = two_locations();
        let opt_a = weather_chart_option(
            "Osaka",
            &all[0].1.clone(),
            &all,
            false,
            &crate::echarts::ChartColors::default(),
        );
        let opt_b = weather_chart_option(
            "Palma",
            &[hour(2026, 7, 9, 14, 10.0, 0)],
            &all,
            false,
            &crate::echarts::ChartColors::default(),
        );
        // "Nijar" is index 1 in both charts' series-from-`all`; in opt_b the
        // own series is prepended, shifting it to position 2 — same color.
        assert_eq!(
            opt_a["series"][1]["color"],
            opt_b["series"][2]["color"],
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p chaos-ui weather`
Expected: COMPILE ERROR — `y_range` not found, `weather_chart_option` takes 3 arguments.

- [ ] **Step 3: Implement `y_range`, the palette, and the new builder**

In `crates/chaos-ui/src/pages/weather.rs`, add below `hourly_temps`:

```rust
/// Marker colors for sibling locations in the combined tooltip, cycled by
/// each location's index in the page-wide list so a place keeps one color
/// on every chart. (Its line is invisible; only the tooltip marker shows.)
const SIBLING_PALETTE: [&str; 6] = [
    "#5470c6", "#91cc75", "#fac858", "#ee6666", "#73c0de", "#9a60b4",
];

/// The page-wide y-axis bounds: min/max over every location's converted
/// temperatures, padded one display degree and rounded outward. Every chart
/// pins its y-axis to this, so scales match and zooming never rescales.
/// Precondition: at least one non-empty series (callers guard empty hourly).
fn y_range(everyone: &[(&str, &[chaos_domain::HourlyForecast])], fahrenheit: bool) -> (f64, f64) {
    let temps = everyone.iter().flat_map(|(_, h)| hourly_temps(h, fahrenheit));
    let (min, max) = temps.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), t| {
        (lo.min(t), hi.max(t))
    });
    ((min - 1.0).floor(), (max + 1.0).ceil())
}
```

Then replace the whole `weather_chart_option` function with:

```rust
/// The ECharts option for one location's 48 h forecast. Category x-axis (one
/// slot per hour) so each tick carries an emoji-over-hour label and cross-
/// location zoom aligns by forecast hour, not wall-clock. Every loaded
/// location rides along as an extra series with an invisible line, which
/// makes the built-in axis tooltip list all of them (the JSON option bridge
/// can't carry a JS formatter) — and the y-axis is pinned to the page-wide
/// range so charts compare at a glance. Colours are injected by the caller
/// so this stays pure/testable.
fn weather_chart_option(
    own_name: &str,
    own_hourly: &[chaos_domain::HourlyForecast],
    all: &[(String, Vec<chaos_domain::HourlyForecast>)],
    fahrenheit: bool,
    colors: &crate::echarts::ChartColors,
) -> serde_json::Value {
    let unit = if fahrenheit { "°F" } else { "°C" };
    let labels = hourly_labels(own_hourly);

    let text = colors.text.as_str();
    let muted = colors.muted.as_str();
    let border = colors.border.as_str();
    let surface = colors.surface.as_str();
    let accent = colors.accent.as_str();

    // The shared list is filled by row Effects, so this row's own entry may
    // not have landed yet — fold it into `everyone` (for the y-range and a
    // prepended series) so the chart never renders lineless.
    let mut everyone: Vec<(&str, &[chaos_domain::HourlyForecast])> = all
        .iter()
        .map(|(name, hourly)| (name.as_str(), hourly.as_slice()))
        .collect();
    let own_missing = !everyone.iter().any(|(name, _)| *name == own_name);
    if own_missing {
        everyone.insert(0, (own_name, own_hourly));
    }
    let (y_min, y_max) = y_range(&everyone, fahrenheit);

    let own_series = |hourly: &[chaos_domain::HourlyForecast]| {
        serde_json::json!({
            "name": own_name,
            "type": "line",
            "showSymbol": false,
            "color": accent,
            "lineStyle": { "width": 1.5 },
            "data": hourly_temps(hourly, fahrenheit),
        })
    };
    // Palette indices come from the SHARED list (`all`), never the merged
    // one, so a sibling's marker color is identical on every chart even
    // when a not-yet-published row prepends its own series.
    let mut series: Vec<serde_json::Value> = all
        .iter()
        .enumerate()
        .map(|(i, (name, hourly))| {
            if name == own_name {
                own_series(hourly)
            } else {
                // Line hidden, series silent; the color survives as the
                // tooltip marker. Aligned by hour index (same equal-length
                // assumption as the cross-location zoom sync).
                serde_json::json!({
                    "name": name,
                    "type": "line",
                    "showSymbol": false,
                    "silent": true,
                    "color": SIBLING_PALETTE[i % SIBLING_PALETTE.len()],
                    "lineStyle": { "opacity": 0 },
                    "data": hourly_temps(hourly, fahrenheit),
                })
            }
        })
        .collect();
    if own_missing {
        series.insert(0, own_series(own_hourly));
    }

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
        // Rendered but transparent + off-canvas so its dataZoomSelect cursor
        // exists for ChartCanvas to arm (see the Home chart for the rationale).
        "toolbox": {
            "show": true,
            "top": -40,
            "feature": { "dataZoom": { "yAxisIndex": "none", "iconStyle": { "opacity": 0 } } },
        },
        "dataZoom": [{
            "type": "inside",
            "xAxisIndex": 0,
            "zoomOnMouseWheel": false,
            "moveOnMouseMove": false,
            "moveOnMouseWheel": false,
        }],
        "xAxis": {
            "type": "category",
            "data": labels,
            "axisLabel": { "color": muted, "interval": 2, "lineHeight": 16 },
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

Note the deliberate changes from the old builder: `yAxis.scale: true` is gone (replaced by pinned `min`/`max`), and `series` is the computed vector.

- [ ] **Step 4: Update the call site in `weather_row_body`**

In `weather_row_body`, the head `view!` consumes `weather.location` by move, so the clone must happen **before it** — add this as the first line of the function body (before the `details` binding):

```rust
    let location = weather.location.clone();
```

Then replace the chart block at the bottom (the `{ let hourly = weather.hourly; ... }` expression) with:

```rust
        {
            let hourly = weather.hourly;
            if hourly.is_empty() {
                // Mirror the Home chart's empty state rather than showing an
                // axes-only, line-less box.
                view! { <p class="muted">"No hourly forecast."</p> }.into_any()
            } else {
                let colors = crate::echarts::ChartColors::from_theme();
                // Task 3 replaces the empty list with the page-wide signal.
                let option = Callback::new(move |()| {
                    weather_chart_option(&location, &hourly, &[], fahrenheit, &colors)
                });
                view! {
                    <crate::echarts::ChartCanvas option group="weather" class="weather-chart"/>
                }
                .into_any()
            }
        }
```

Watch the ordering: `let location = weather.location.clone();` must come before `let hourly = weather.hourly;` only if `weather` is still whole — it is; both lines are fine in that order. (`weather.location` was already moved into the head `view!` above? No — the head view uses `{weather.location}` by move. Check the actual code: the head `view!` consumes `weather.location`. If so, clone it **before** the head view: add `let location = weather.location.clone();` as the first line of `weather_row_body`, and use `location` in the closure.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p chaos-ui`
Expected: PASS — all weather tests (4 kept + 4 new) plus the 6 echarts tests.

- [ ] **Step 6: Run the full gate**

Run: `just check`
Expected: green. `SIBLING_PALETTE` and `y_range` are both used by `weather_chart_option`, which is used by `weather_row_body` — no dead code.

- [ ] **Step 7: Commit**

```bash
git add crates/chaos-ui/src/pages/weather.rs
git commit -m "feat(weather): combined tooltip series + shared fixed y-range

Every weather chart embeds all loaded locations as series - its own drawn
in accent, siblings with an invisible line but a stable palette marker -
so the native axis tooltip lists every location's temperature. The y-axis
is pinned to the page-wide min/max (+-1 degree, rounded outward), fixed
under zoom. The call site still passes an empty sibling list; the shared
signal lands next.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_019emSFGLm8F1AFZADq2fQA3"
```

---

### Task 3: Page-wide shared forecast signal

**Files:**
- Modify: `crates/chaos-ui/src/pages/weather.rs` (WeatherPage, WeatherRow, weather_row_body)

Context: pure wiring — no new pure logic, so no new unit tests; correctness is compile + `just check` + the manual smoke test (Task 4). The signal type appears in three signatures; alias it.

- [ ] **Step 1: Add the type alias**

Near the top of `crates/chaos-ui/src/pages/weather.rs` (below the `use` lines):

```rust
/// Every loaded location's hourly forecast, insertion-ordered as fetches
/// resolve; keyed by the API's resolved display name. Charts read it for
/// the combined tooltip and the shared y-range.
type LoadedForecasts = Vec<(String, Vec<chaos_domain::HourlyForecast>)>;
```

And change `weather_chart_option`'s parameter to use it: `all: &LoadedForecasts` → note `&LoadedForecasts` derefs to the same slice ops used in Task 2 (`all.iter()`), so the body doesn't change. Update the tests' calls if the compiler complains (it won't: `&Vec<_>` coerces where `&[_]` was expected, but the parameter type is now the alias — passing `&all` where `all: Vec<...>` still works).

- [ ] **Step 2: Create the signal in `WeatherPage` and pass it down**

In `WeatherPage`, after `let input = RwSignal::new(String::new());`:

```rust
    let loaded = RwSignal::new(LoadedForecasts::new());
```

Update both `WeatherRow` instantiations inside the `move ||` list closure:

```rust
                if list.is_empty() {
                    // Same place the dashboard widget shows.
                    view! { <WeatherRow location=None on_remove=None loaded/> }.into_any()
                } else {
                    list.into_iter()
                        .map(|place| {
                            view! {
                                <WeatherRow location=Some(place) on_remove=Some(remove) loaded/>
                            }
                        })
                        .collect_view()
                        .into_any()
                }
```

- [ ] **Step 3: Publish each row's forecast into the signal**

In `WeatherRow`, add the prop and the publish/cleanup logic. New signature:

```rust
#[component]
fn WeatherRow(
    location: Option<String>,
    on_remove: Option<Callback<String>>,
    loaded: RwSignal<LoadedForecasts>,
) -> impl IntoView {
```

After the `let data = LocalResource::new(...)` block, add:

```rust
    // Publish this row's forecast into the page-wide list (charts read it
    // for the combined tooltip and shared y-range). Upsert by resolved name
    // so refetches don't duplicate; remember the name to unpublish when the
    // row unmounts (location removed / page left).
    let published = StoredValue::new(None::<String>);
    Effect::new(move |_| {
        let Some(Ok(weather)) = data.get() else {
            return;
        };
        if weather.hourly.is_empty() {
            return;
        }
        let (name, hourly) = (weather.location, weather.hourly);
        published.set_value(Some(name.clone()));
        loaded.update(|list| match list.iter_mut().find(|(n, _)| *n == name) {
            Some(entry) => entry.1 = hourly.clone(),
            None => list.push((name.clone(), hourly.clone())),
        });
    });
    on_cleanup(move || {
        if let Some(name) = published.get_value() {
            loaded.update(|list| list.retain(|(n, _)| n != &name));
        }
    });
```

(`data.get()` clones the `WeatherData`, so moving `location`/`hourly` out of it is fine. The clones inside `update` are needed because the closure only borrows them.)

- [ ] **Step 4: Feed the signal to the option builder**

`weather_row_body` gains the signal parameter. Change its signature:

```rust
fn weather_row_body(weather: WeatherData, loaded: RwSignal<LoadedForecasts>) -> impl IntoView {
```

Update its call in `WeatherRow`'s view:

```rust
                Some(Ok(weather)) => weather_row_body(weather, loaded).into_any(),
```

And in the chart block from Task 2, replace the `&[]` placeholder — reading the signal inside the callback makes `ChartCanvas` re-render the chart whenever any sibling loads or unloads:

```rust
                let option = Callback::new(move |()| {
                    let all = loaded.get();
                    weather_chart_option(&location, &hourly, &all, fahrenheit, &colors)
                });
```

(Also delete the now-stale "Task 3 replaces…" comment.)

- [ ] **Step 5: Run tests and the full gate**

Run: `cargo test -p chaos-ui && just check`
Expected: all tests PASS, gate green.

- [ ] **Step 6: Commit**

```bash
git add crates/chaos-ui/src/pages/weather.rs
git commit -m "feat(weather): page-wide forecast signal feeds every chart

WeatherPage owns a LoadedForecasts signal; each row upserts its resolved
forecast on load and retracts it on unmount. Chart options read the signal
reactively, so the combined tooltip and the shared y-range update as
sibling locations stream in.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_019emSFGLm8F1AFZADq2fQA3"
```

---

### Task 4: Manual smoke test (no code)

Serve the app (backend on 4600, frontend on all interfaces for a remote browser):

```bash
# Terminal 1
just server
# Terminal 2
cd crates/chaos-web && trunk serve --address 0.0.0.0 --port 60000
```

Verify in the browser at `http://<host>:60000/`:

- [ ] Weather tab with ≥2 locations: hovering any chart shows ONE tooltip listing every location's temperature (own + siblings, each with a colored marker), and the synced tooltip line moves on the sibling charts.
- [ ] All weather charts show the identical y-axis range; zooming does not rescale it.
- [ ] Drag-select a zoom on one weather chart → all zoom together. Double-click → all step back out; repeated double-clicks reach the full 48 h.
- [ ] Home tab: drag-zoom the temperature chart, then double-click → it steps back out (and weather charts are unaffected).
- [ ] Phone (or devtools touch emulation): pinch-in zooms, pinch-out zooms back out progressively.
- [ ] A sibling's invisible line never shows on another chart; adding/removing a location updates every chart's tooltip list and y-range.
