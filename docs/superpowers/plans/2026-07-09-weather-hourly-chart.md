# Weather Hourly Forecast Chart Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Weather tab's hourly text strip with an ECharts temperature line chart whose x-axis shows a weather emoji above each date/hour, with pan/zoom and all location charts synchronized.

**Architecture:** Extract a reusable `ChartCanvas` component into `chaos-ui/src/echarts.rs` (init → set_option → drag-zoom arming → resize → dispose, plus optional ECharts connect-group). `TemperatureChart` (Home) migrates onto it. A new `WeatherChart` in `weather.rs` builds a category-axis option from pure, unit-tested helpers and joins the `"weather"` connect group so charts sync.

**Tech Stack:** Rust workspace (Leptos 0.8 CSR), Apache ECharts 5.6.0 (self-hosted), hand-written wasm-bindgen glue. Native unit tests via `cargo nextest`; wasm compile via `just check`.

**Branch:** work on `feat/weather-hourly-chart` (create it first — see Task 0). Every commit message ends with the repo's `Co-Authored-By:` + `Claude-Session:` trailers (copy them from a recent `git log` entry). Run checks inside `nix develop` / direnv.

Spec: `docs/superpowers/specs/2026-07-09-weather-hourly-chart-design.md`

---

### Task 0: Branch

- [ ] **Step 1: Create the feature branch and commit the spec**

```bash
cd /projects/rust/chaos
git checkout -b feat/weather-hourly-chart
git add docs/superpowers/specs/2026-07-09-weather-hourly-chart-design.md docs/superpowers/plans/2026-07-09-weather-hourly-chart.md
git commit -m "docs: weather hourly chart spec + plan"
```

(Trailers: append the `Co-Authored-By:` and `Claude-Session:` lines exactly as they appear in recent commits.)

---

### Task 1: ECharts bindings for connect-group sync

Add the two bindings needed to synchronize charts: a setter for the instance's
`group` property and the module-level `echarts.connect(group)`.

**Files:**
- Modify: `crates/chaos-ui/src/echarts.rs` (inside the `extern "C"` block, ~line 8-26)

- [ ] **Step 1: Add the bindings**

In the `#[wasm_bindgen] extern "C" { ... }` block in `crates/chaos-ui/src/echarts.rs`,
add after the existing `dispose` method (before the closing `}` of the block):

```rust
    /// Assign the chart to a connect-group (charts sharing a group can be
    /// linked with `connect`).
    #[wasm_bindgen(method, setter = group)]
    pub fn set_group(this: &EChart, group: &str);
```

and, still inside the same `extern "C"` block, add the free function:

```rust
    /// `echarts.connect(group)` — link dataZoom + tooltip across every chart
    /// currently assigned to `group`.
    #[wasm_bindgen(js_namespace = echarts)]
    pub fn connect(group: &str);
```

- [ ] **Step 2: Verify it compiles for wasm**

Run: `just check`
Expected: PASS (no new warnings/errors). This is the only way to check wasm-bindgen
extern signatures — they have no native test.

- [ ] **Step 3: Commit**

```bash
git add crates/chaos-ui/src/echarts.rs
git commit -m "feat(echarts): add group setter + connect binding"
```

---

### Task 2: Move `css_var` into `echarts.rs`

`css_var` is private in `home.rs`; the weather chart needs it too. Move it to
`echarts.rs` as `pub(crate)` and update Home to use it.

**Files:**
- Modify: `crates/chaos-ui/src/echarts.rs` (append fn + needed imports)
- Modify: `crates/chaos-ui/src/pages/home.rs` (delete local `css_var` at ~line 373-382, call `crate::echarts::css_var`)

- [ ] **Step 1: Add `css_var` and `ChartColors` to `echarts.rs`**

Append to `crates/chaos-ui/src/echarts.rs`:

```rust
/// A CSS custom property from the active theme (empty string if unset). Reads
/// the DOM, so browser-only — calling it off-wasm panics (wasm-bindgen imports
/// can't run natively). Keep it out of anything unit-tested; inject colours via
/// `ChartColors` instead.
pub(crate) fn css_var(name: &str) -> String {
    web_sys::window()
        .and_then(|w| {
            let body = w.document()?.body()?;
            w.get_computed_style(&body).ok().flatten()
        })
        .and_then(|style| style.get_property_value(name).ok())
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}

/// Theme colours pulled from CSS variables, injected into option builders so
/// those builders stay pure (no DOM) and unit-testable off-wasm.
#[derive(Debug, Default, Clone)]
pub(crate) struct ChartColors {
    pub text: String,
    pub muted: String,
    pub border: String,
    pub surface: String,
    pub accent: String,
}

impl ChartColors {
    /// Read from the active theme (browser only — calls `css_var`).
    pub(crate) fn from_theme() -> Self {
        Self {
            text: css_var("--text"),
            muted: css_var("--muted"),
            border: css_var("--border"),
            surface: css_var("--surface"),
            accent: css_var("--accent"),
        }
    }
}
```

- [ ] **Step 2: Delete the copy in `home.rs` and update call sites**

In `crates/chaos-ui/src/pages/home.rs`, delete the entire `fn css_var(...) { ... }`
(the `/// A CSS custom property ...` doc comment through its closing brace, ~lines
371-382). Then update the four call sites in `chart_option` (~lines 277-280) from
`css_var("--text")` to `crate::echarts::css_var("--text")` (and likewise `--muted`,
`--border`, `--surface`):

```rust
    let text = crate::echarts::css_var("--text");
    let muted = crate::echarts::css_var("--muted");
    let border = crate::echarts::css_var("--border");
    let surface = crate::echarts::css_var("--surface");
```

- [ ] **Step 3: Verify wasm compile**

Run: `just check`
Expected: PASS (no unused-function or unresolved-path errors).

- [ ] **Step 4: Commit**

```bash
git add crates/chaos-ui/src/echarts.rs crates/chaos-ui/src/pages/home.rs
git commit -m "refactor(echarts): share css_var across chart pages"
```

---

### Task 3: `ChartCanvas` reusable mount component

Extract the ECharts mount lifecycle into one component with optional connect-group
support.

**Files:**
- Modify: `crates/chaos-ui/src/echarts.rs` (add imports + `ChartCanvas` component)

- [ ] **Step 1: Add imports at the top of `echarts.rs`**

At the top of `crates/chaos-ui/src/echarts.rs`, below the existing
`use wasm_bindgen::prelude::*;`, add:

```rust
use leptos::prelude::*;
```

- [ ] **Step 2: Add the `ChartCanvas` component**

Append to `crates/chaos-ui/src/echarts.rs`:

```rust
/// A mounted ECharts instance: owns init, option updates, drag-select zoom
/// arming, resize, and disposal. `option` is re-run reactively, so a builder
/// that reads signals re-renders the chart. When `group` is set, the chart
/// joins that ECharts connect-group (shared dataZoom + tooltip across every
/// chart in the group). `class` sizes the container (e.g. `"temp-chart"`).
#[component]
pub fn ChartCanvas(
    option: Callback<(), serde_json::Value>,
    // `into` lets callers pass `group="weather"` (wrapped to Some) or omit it (None).
    #[prop(optional, into)] group: Option<&'static str>,
    class: &'static str,
) -> impl IntoView {
    let node = NodeRef::<leptos::html::Div>::new();
    let chart = StoredValue::new_local(None::<EChart>);
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
        let _ = instance.set_option(&opt);
        // Arm drag-select zoom (a toolbox feature, armed programmatically so no
        // toolbox icon must be clicked — the toolbox itself stays hidden).
        let _ = instance.dispatch_action(&json(
            r#"{"type":"takeGlobalCursor","key":"dataZoomSelect","dataZoomSelectActive":true}"#,
        ));
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

- [ ] **Step 3: Verify wasm compile**

Run: `just check`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/chaos-ui/src/echarts.rs
git commit -m "feat(echarts): ChartCanvas mount component with connect-group"
```

---

### Task 4: Migrate Home's `TemperatureChart` onto `ChartCanvas`

Replace the hand-rolled mount in `TemperatureChart` with `ChartCanvas`, proving the
extraction before the weather chart depends on it.

**Files:**
- Modify: `crates/chaos-ui/src/pages/home.rs` (`TemperatureChart`, ~lines 180-244)

- [ ] **Step 1: Replace the body of `TemperatureChart`**

In `crates/chaos-ui/src/pages/home.rs`, replace the whole `TemperatureChart` component
(from `#[component]\nfn TemperatureChart(` through its closing `}` including the
`.into_any()`, ~lines 180-244) with:

```rust
#[component]
fn TemperatureChart(
    series: Vec<TemperatureSeries>,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> impl IntoView {
    if !series.iter().any(|s| !s.readings.is_empty()) {
        return view! { <p class="muted">"No readings in this range."</p> }.into_any();
    }

    let option = Callback::new(move |()| chart_option(&series, start, end));
    view! { <crate::echarts::ChartCanvas option class="temp-chart"/> }.into_any()
}
```

Note: `series`/`start`/`end` are moved into the `Callback`; `chart_option` is unchanged.
The Home chart is **not** in a connect-group (no `group` prop), so it keeps its own
independent zoom.

- [ ] **Step 2: Remove now-unused imports**

If the compiler reports unused imports in `home.rs` (e.g. `NodeRef` usage removed),
delete only what clippy/`just check` flags. `Callback`/`Effect` come from
`leptos::prelude::*` which stays.

- [ ] **Step 3: Verify wasm compile + native tests still pass**

Run: `just check`
Expected: PASS, no unused-import warnings.

Run: `cargo nextest run -p chaos-ui`
Expected: PASS (no behavior change).

- [ ] **Step 4: Manually verify Home chart still works**

Run the app (`just desktop` against a running server, or the web dev flow) and confirm
the Home temperature chart still renders, tooltips, and drag-zooms.

- [ ] **Step 5: Commit**

```bash
git add crates/chaos-ui/src/pages/home.rs
git commit -m "refactor(home): mount TemperatureChart via ChartCanvas"
```

---

### Task 5: Pure helpers — hourly labels + temperatures (TDD)

Build the two pure functions the weather option needs, test-first. These carry all the
emoji/label/conversion logic and are native-testable.

**Files:**
- Modify: `crates/chaos-ui/src/pages/weather.rs` (add helpers + `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing tests**

Append to `crates/chaos-ui/src/pages/weather.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chaos_domain::HourlyForecast;
    use chrono::NaiveDate;

    fn hour(y: i32, m: u32, d: u32, h: u32, temp_c: f64, code: i32) -> HourlyForecast {
        HourlyForecast {
            time: NaiveDate::from_ymd_opt(y, m, d)
                .unwrap()
                .and_hms_opt(h, 0, 0)
                .unwrap(),
            temp_c,
            weather_code: code,
        }
    }

    #[test]
    fn labels_show_emoji_over_hour() {
        // 2026-07-09 is a Thursday.
        let hours = vec![hour(2026, 7, 9, 14, 20.0, 0)];
        assert_eq!(hourly_labels(&hours), vec!["☀️\n14h".to_string()]);
    }

    #[test]
    fn labels_show_weekday_at_midnight() {
        // 2026-07-10 is a Friday; midnight rows carry the weekday, not "0h".
        let hours = vec![hour(2026, 7, 10, 0, 15.0, 2)];
        assert_eq!(hourly_labels(&hours), vec!["⛅\nFri".to_string()]);
    }

    #[test]
    fn temps_convert_and_round_celsius() {
        let hours = vec![hour(2026, 7, 9, 14, 20.04, 0)];
        assert_eq!(hourly_temps(&hours, false), vec![20.0]);
    }

    #[test]
    fn temps_convert_and_round_fahrenheit() {
        // 20°C -> 68°F.
        let hours = vec![hour(2026, 7, 9, 14, 20.0, 0)];
        assert_eq!(hourly_temps(&hours, true), vec![68.0]);
    }
}
```

- [ ] **Step 2: Run the tests to confirm they fail**

Run: `cargo nextest run -p chaos-ui`
Expected: FAIL — `cannot find function hourly_labels` / `hourly_temps`.

- [ ] **Step 3: Implement the helpers**

Add near the top of `crates/chaos-ui/src/pages/weather.rs`, after the imports (keep the
existing `use chrono::Timelike;` — it is used here):

```rust
/// Two-line x-axis labels: weather emoji on top, then the hour (`"14h"`), or
/// the weekday (`"Fri"`) at midnight so day boundaries read at a glance.
fn hourly_labels(hourly: &[chaos_domain::HourlyForecast]) -> Vec<String> {
    hourly
        .iter()
        .map(|h| {
            let below = if h.time.hour() == 0 {
                h.time.format("%a").to_string()
            } else {
                format!("{}h", h.time.hour())
            };
            format!("{}\n{}", crate::weather_emoji(h.weather_code), below)
        })
        .collect()
}

/// Temperatures in the display unit, one decimal (values land verbatim in the
/// chart tooltip).
fn hourly_temps(hourly: &[chaos_domain::HourlyForecast], fahrenheit: bool) -> Vec<f64> {
    hourly
        .iter()
        .map(|h| {
            let value = if fahrenheit {
                h.temp_c * 9.0 / 5.0 + 32.0
            } else {
                h.temp_c
            };
            (value * 10.0).round() / 10.0
        })
        .collect()
}
```

- [ ] **Step 4: Run the tests to confirm they pass**

Run: `cargo nextest run -p chaos-ui`
Expected: PASS (4 new tests).

- [ ] **Step 5: Commit**

```bash
git add crates/chaos-ui/src/pages/weather.rs
git commit -m "feat(weather): pure hourly label + temperature helpers"
```

---

### Task 6: Weather chart option builder

Assemble the ECharts option JSON (category axis, two-line labels, single line,
tooltip, zoom) from the Task 5 helpers.

**Files:**
- Modify: `crates/chaos-ui/src/pages/weather.rs` (add `weather_chart_option` + a test)

- [ ] **Step 1: Write a failing structural test**

Add these tests inside the existing `mod tests` in `weather.rs` (alongside the Task 5
tests):

```rust
    #[test]
    fn option_has_category_axis_and_one_series() {
        let hours = vec![hour(2026, 7, 9, 14, 20.0, 0), hour(2026, 7, 9, 15, 21.0, 2)];
        // Empty colours are fine — the test asserts structure, not styling, and
        // this keeps the DOM-reading `css_var` out of the native test.
        let opt = weather_chart_option(&hours, false, &crate::echarts::ChartColors::default());
        assert_eq!(opt["xAxis"]["type"], "category");
        // Every-3rd-hour cadence.
        assert_eq!(opt["xAxis"]["axisLabel"]["interval"], 2);
        // One temperature line, data aligned to the categories.
        assert_eq!(opt["series"].as_array().unwrap().len(), 1);
        assert_eq!(opt["series"][0]["data"], serde_json::json!([20.0, 21.0]));
        assert_eq!(opt["xAxis"]["data"], serde_json::json!(["☀️\n14h", "⛅\n15h"]));
    }
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo nextest run -p chaos-ui`
Expected: FAIL — `cannot find function weather_chart_option`.

- [ ] **Step 3: Implement `weather_chart_option`**

Add to `crates/chaos-ui/src/pages/weather.rs` (after the helpers from Task 5). It is fully
pure — theme colours arrive via `&ChartColors`, so the test above needs no `web_sys`:

```rust
/// The ECharts option for one location's 48 h forecast. Category x-axis (one
/// slot per hour) so each tick carries an emoji-over-hour label and cross-
/// location zoom aligns by forecast hour, not wall-clock. Colours are injected
/// by the caller so this stays pure/testable.
fn weather_chart_option(
    hourly: &[chaos_domain::HourlyForecast],
    fahrenheit: bool,
    colors: &crate::echarts::ChartColors,
) -> serde_json::Value {
    let unit = if fahrenheit { "°F" } else { "°C" };
    let labels = hourly_labels(hourly);
    let temps = hourly_temps(hourly, fahrenheit);

    let text = colors.text.as_str();
    let muted = colors.muted.as_str();
    let border = colors.border.as_str();
    let surface = colors.surface.as_str();
    let accent = colors.accent.as_str();

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
            "scale": true,
            "axisLabel": { "color": muted, "formatter": format!("{{value}}{unit}") },
            "splitLine": { "lineStyle": { "color": border } },
        },
        "series": [{
            "name": "Temperature",
            "type": "line",
            "showSymbol": false,
            "color": accent,
            "lineStyle": { "width": 1.5 },
            "data": temps,
        }],
    })
}
```

- [ ] **Step 4: Run the tests to confirm they pass**

Run: `cargo nextest run -p chaos-ui`
Expected: PASS (5 tests total in `weather.rs`).

- [ ] **Step 5: Verify wasm compile**

Run: `just check`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/chaos-ui/src/pages/weather.rs
git commit -m "feat(weather): ECharts option builder for hourly forecast"
```

---

### Task 7: Swap the strip for the chart in the UI

Render the chart via `ChartCanvas` in the `"weather"` connect-group, replacing the
`.hourly-strip` markup in `weather_row_body`.

**Files:**
- Modify: `crates/chaos-ui/src/pages/weather.rs` (`weather_row_body`, ~lines 133-153)

- [ ] **Step 1: Replace the hourly-strip block**

In `weather_row_body` in `crates/chaos-ui/src/pages/weather.rs`, replace the entire
`<div class="hourly-strip"> ... </div>` block (from `<div class="hourly-strip">`
through its matching `</div>`, ~lines 133-153) with:

```rust
        {
            let hourly = weather.hourly;
            let colors = crate::echarts::ChartColors::from_theme();
            let option = Callback::new(move |()| weather_chart_option(&hourly, fahrenheit, &colors));
            view! {
                <crate::echarts::ChartCanvas option group="weather" class="weather-chart"/>
            }
        }
```

`fahrenheit` is the `let fahrenheit = crate::weather_fahrenheit();` already at the top
of `weather_row_body`; `weather.hourly` and `colors` are moved into the option builder
(read once here, in the browser, before the pure builder runs). Ensure
`use leptos::prelude::*;` is in scope (it is, at the top of `weather.rs`) for
`Callback`.

- [ ] **Step 2: Verify wasm compile**

Run: `just check`
Expected: PASS. If clippy flags `weather.hourly` being moved while other fields of
`weather` are read earlier, confirm all earlier reads (in the header) happen before
this block — they do (header is built first in `weather_row_body`).

- [ ] **Step 3: Commit**

```bash
git add crates/chaos-ui/src/pages/weather.rs
git commit -m "feat(weather): render hourly forecast as synchronized chart"
```

---

### Task 8: Styles — drop strip CSS, add `.weather-chart`

**Files:**
- Modify: `crates/chaos-web/styles.css` (remove strip rules ~lines 1278-1309; add `.weather-chart`)

- [ ] **Step 1: Remove the strip rules**

In `crates/chaos-web/styles.css`, delete the four rules for `.hourly-strip`,
`.hour-cell`, `.hour-cell.day-break`, `.hour-cell.day-break .hour-label`, and
`.hour-label` (the block spanning ~lines 1278-1309, from `.hourly-strip {` through the
closing brace of `.hour-label { ... }`).

- [ ] **Step 2: Add `.weather-chart`**

Next to the existing `.temp-chart` rule (~line 1763), add:

```css
.weather-chart {
  height: 260px;
  margin-top: 0.75rem;
}

@media (max-width: 40rem) {
  .weather-chart {
    height: 200px;
  }
}
```

- [ ] **Step 3: Verify the bundle builds**

Run: `just build-web`
Expected: PASS (trunk builds; no missing-class errors — CSS isn't type-checked, so this
just confirms the bundle still compiles).

- [ ] **Step 4: Commit**

```bash
git add crates/chaos-web/styles.css
git commit -m "style(weather): chart sizing, drop hourly-strip rules"
```

---

### Task 9: Full verification

- [ ] **Step 1: Full check + tests**

Run: `just check`
Expected: PASS (fmt, clippy `-D warnings`, native + wasm compile).

Run: `cargo nextest run --workspace`
Expected: PASS (all tests, including the 5 new weather tests).

- [ ] **Step 2: Manual end-to-end verification**

Run the app (`just desktop` against a running server). On the Weather tab:
- Add two locations (e.g. `Paris` and `Osaka, JP`).
- Confirm each shows the current-conditions header plus a temperature line chart.
- Confirm the x-axis shows a weather emoji above the hour, at every-3rd-hour cadence,
  with the weekday at midnight.
- Drag-select to zoom on one chart → the **other chart zooms to the same range**
  (synchronization). Hover → tooltips track together.

- [ ] **Step 3: Final commit (if any manual fixes were needed)**

```bash
git add -A
git commit -m "fix(weather): adjustments from manual verification"
```

(Skip if nothing changed.)

---

## Notes for the implementer

- **Trailers:** every commit ends with the repo's `Co-Authored-By:` and `Claude-Session:`
  lines — copy them verbatim from a recent `git log` entry.
- **`weather_emoji` returns emoji with variation selectors** (e.g. `"☀️"` = U+2600 U+FE0F).
  The Task 5 test strings must be copied exactly; don't retype the emoji.
- **Why category axis, not time axis:** it lets every tick own a custom two-line label
  and makes cross-location zoom align by forecast-hour index (locations have different
  local clocks via `timezone=auto`). This is intentional — do not switch to a time axis.
- **connect timing:** charts mount as their `LocalResource`s resolve; each calls
  `connect("weather")` after `set_option`, which re-links the whole group as members
  appear. No central coordinator is needed.
