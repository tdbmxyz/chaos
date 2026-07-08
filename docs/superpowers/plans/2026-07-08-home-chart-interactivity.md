# Home Tab Interactive Chart + Light Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Home tab's temperature chart interactive (hover tooltip, legend toggle, drag-zoom, Last 3h) via ECharts, and fix the light card (self-unchecking checkbox, overflowing slider).

**Architecture:** Vendored Apache ECharts driven from Leptos through a thin hand-written wasm-bindgen glue (`chaos-ui/src/echarts.rs`); `TemperatureChart` becomes a `div` + `Effect` that feeds ECharts a serde_json option. The light fix is server-side: `set_light` polls Home Assistant until the commanded state is observed before answering (max 2 s), plus a one-line optimistic signal set in the UI.

**Tech Stack:** Rust workspace (Leptos 0.8 CSR, Axum 0.8), Apache ECharts 5.6.0 (self-hosted JS), wasm-bindgen 0.2.126 (already in Cargo.lock via leptos — the flake's pinned wasm-bindgen-cli stays valid).

**Branch:** work on `feat/home-chart-interactivity` (already created, spec committed). Every commit message ends with the repo's Co-Authored-By + Claude-Session trailers (see recent `git log`). All checks run inside `nix develop` (or direnv).

Spec: `docs/superpowers/specs/2026-07-08-home-tab-chart-and-lights-design.md`

---

### Task 1: Server confirms light commands before answering

The bug: `set_light` calls `turn_on`, then immediately fetches state; HA often still reports `off` (Zigbee confirms asynchronously), so the client's fresh checkbox gets overwritten by a stale `off`.

**Files:**
- Modify: `crates/chaos-server/src/home_assistant.rs` (fn `set_light`, ~line 194; add `confirm` helper + `#[cfg(test)] mod tests` at end of file)

- [ ] **Step 1: Write the failing test (stub HA over a local TCP port)**

Append at the end of `crates/chaos-server/src/home_assistant.rs`:

```rust
#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use chaos_domain::LightCommand;

    use super::*;
    use crate::config::{HomeAssistantConfig, HomeEntityDef};

    fn light_def() -> HomeEntityDef {
        HomeEntityDef {
            id: "lamp".into(),
            label: Some("Lamp".into()),
            entity_id: "light.lamp".into(),
        }
    }

    /// Stub HA: `/api/states/{id}` walks through `states` (sticking on the
    /// last one), `/api/services/light/{service}` always succeeds. Returns
    /// the base URL and a counter of state fetches.
    async fn stub_ha(states: Vec<&'static str>) -> (Url, Arc<AtomicUsize>) {
        let fetches = Arc::new(AtomicUsize::new(0));
        let states = Arc::new(states);
        let app = axum::Router::new()
            .route(
                "/api/states/{id}",
                axum::routing::get({
                    let fetches = fetches.clone();
                    let states = states.clone();
                    move |_: axum::extract::Path<String>| {
                        let n = fetches.fetch_add(1, Ordering::SeqCst);
                        let state = states[n.min(states.len() - 1)];
                        let body =
                            serde_json::json!({ "state": state, "attributes": {} });
                        async move { axum::Json(body) }
                    }
                }),
            )
            .route(
                "/api/services/light/{service}",
                axum::routing::post(|| async { axum::Json(serde_json::json!([])) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding stub ha");
        let addr = listener.local_addr().expect("stub ha addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serving stub ha");
        });
        (
            format!("http://{addr}/").parse().expect("stub ha url"),
            fetches,
        )
    }

    fn client(base_url: Url) -> HomeAssistantClient {
        let token = std::env::temp_dir().join(format!(
            "chaos-ha-test-token-{}-{:p}",
            std::process::id(),
            &base_url
        ));
        std::fs::write(&token, "test-token").expect("writing stub token");
        HomeAssistantClient::new(&HomeAssistantConfig {
            base_url: Some(base_url),
            token_file: Some(token),
            sensors: vec![],
            lights: vec![light_def()],
        })
        .expect("building client")
        .expect("client is configured")
    }

    /// HA reports `off` for a while after turn_on (async confirmation):
    /// set_light must keep polling and answer with the settled `on`.
    #[tokio::test]
    async fn set_light_waits_for_the_commanded_state() {
        let (url, fetches) = stub_ha(vec!["off", "off", "on"]).await;
        let ha = client(url);

        let state = ha
            .set_light(
                &light_def(),
                &LightCommand {
                    on: Some(true),
                    ..Default::default()
                },
            )
            .await
            .expect("set_light");

        assert!(state.on, "should report the confirmed on state");
        assert!(
            fetches.load(Ordering::SeqCst) >= 3,
            "should have polled past the stale readings"
        );
    }

    /// A light that never confirms: after the poll budget the last observed
    /// (real) state is returned rather than hanging or lying.
    #[tokio::test]
    async fn set_light_reports_the_truth_on_confirmation_timeout() {
        let (url, _fetches) = stub_ha(vec!["off"]).await;
        let ha = client(url);

        let state = ha
            .set_light(
                &light_def(),
                &LightCommand {
                    on: Some(true),
                    ..Default::default()
                },
            )
            .await
            .expect("set_light");

        assert!(!state.on, "timeout must surface HA's actual state");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p chaos-server set_light -- --nocapture`
Expected: `set_light_waits_for_the_commanded_state` FAILS (assertion `state.on` — the current code returns the first, stale fetch). The timeout test may pass already; that's fine.

- [ ] **Step 3: Implement the confirm poll**

In `crates/chaos-server/src/home_assistant.rs`, add below the `TIMEOUT` const:

```rust
/// After a light command, HA can report the old state for a moment
/// (Zigbee and friends confirm asynchronously). Poll until the command
/// is observed so clients never receive a stale state.
const CONFIRM_POLLS: usize = 8;
const CONFIRM_INTERVAL: Duration = Duration::from_millis(250);
```

Replace the whole `set_light` method with:

```rust
    pub async fn set_light(
        &self,
        def: &HomeEntityDef,
        cmd: &LightCommand,
    ) -> Result<LightState, String> {
        match cmd.on {
            Some(false) => {
                self.call_service("turn_off", def, None, None).await?;
                self.confirm(def, |state| !state.on).await
            }
            Some(true) => {
                self.call_service("turn_on", def, cmd.brightness, cmd.color)
                    .await?;
                self.confirm(def, |state| state.on).await
            }
            // Adjustments only apply to a lit lamp: HA's turn_on would
            // power the light as a side effect of a brightness/color
            // change, so an off light is left untouched.
            None => {
                let state = self.fetch_light_state(def).await?;
                if !state.on {
                    return Ok(state);
                }
                self.call_service("turn_on", def, cmd.brightness, cmd.color)
                    .await?;
                let target = cmd.brightness;
                self.confirm(def, move |state| match target {
                    // brightness_pct → 0-255 → pct roundtrips lossily,
                    // hence the tolerance.
                    Some(pct) => state
                        .brightness
                        .is_some_and(|b| b.abs_diff(pct) <= 2),
                    None => true,
                })
                .await
            }
        }
    }

    /// Poll the entity until `settled` observes the commanded state, up to
    /// CONFIRM_POLLS × CONFIRM_INTERVAL; then return the last state seen —
    /// a genuinely failed command still reports the truth. Poll errors are
    /// ignored (the last good reading wins).
    async fn confirm(
        &self,
        def: &HomeEntityDef,
        settled: impl Fn(&LightState) -> bool,
    ) -> Result<LightState, String> {
        let mut state = self.fetch_light_state(def).await?;
        for _ in 0..CONFIRM_POLLS {
            if settled(&state) {
                return Ok(state);
            }
            tokio::time::sleep(CONFIRM_INTERVAL).await;
            if let Ok(fresh) = self.fetch_light_state(def).await {
                state = fresh;
            }
        }
        Ok(state)
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p chaos-server set_light`
Expected: both tests PASS (the timeout test takes ~2 s — that's the poll budget).

- [ ] **Step 5: Full check + commit**

Run: `just check && cargo test --workspace` — all green.

```bash
git add crates/chaos-server/src/home_assistant.rs
git commit -m "fix(home): confirm light commands against HA before answering"
```

---

### Task 2: Optimistic checkbox + slider overflow (UI/CSS)

**Files:**
- Modify: `crates/chaos-ui/src/pages/home.rs` (the `toggle` closure in `LightCard`)
- Modify: `crates/chaos-web/styles.css` (`.light-card-row` block, ~line 1848)

- [ ] **Step 1: Set the `on` signal optimistically in `toggle`**

In `LightCard`'s `toggle` closure (crates/chaos-ui/src/pages/home.rs), add `on.set(checked);` right after reading the checkbox, so dependent state (brightness/color adjustment routing) is coherent while the command is in flight:

```rust
    let toggle = {
        let send = send.clone();
        move |ev: leptos::ev::Event| {
            let checked = event_target_checked(&ev);
            // Optimistic: the card follows the user's intent immediately;
            // apply_state reconciles from the (now confirmed) response.
            on.set(checked);
            let mut cmd = LightCommand {
                on: Some(checked),
                ..Default::default()
            };
            if checked {
                let queued = pending.get_untracked();
                cmd.brightness = queued.brightness;
                cmd.color = queued.color;
                pending.set(LightCommand::default());
            }
            send(cmd);
        }
    };
```

- [ ] **Step 2: Fix the slider overflow in CSS**

In `crates/chaos-web/styles.css`, replace:

```css
.light-card-row input[type="range"] {
  flex: 1;
}
```

with:

```css
.light-card-row input[type="range"] {
  flex: 1;
  /* range inputs have an intrinsic ~130px minimum that flex can't shrink */
  min-width: 0;
}

/* Fixed slot so "5%" vs "100%" never reflows the row. */
.light-card-row > .muted:last-child {
  flex: none;
  min-width: 3.2rem;
  text-align: right;
  font-variant-numeric: tabular-nums;
}
```

Note: the Color row's last child is also a `.muted`? It is not — its children are `span.muted` + `input[type=color]`, so `:last-child` only matches the Brightness row's percentage span. The Brightness row is `span.muted`, `input`, `span.muted` — the selector hits the trailing span only.

- [ ] **Step 3: Check + commit**

Run: `just check`
Expected: clean.

```bash
git add crates/chaos-ui/src/pages/home.rs crates/chaos-web/styles.css
git commit -m "fix(home): optimistic light toggle, contain the brightness slider"
```

---

### Task 3: "Last 3h" quick-range button

**Files:**
- Modify: `crates/chaos-ui/src/pages/home.rs` (`DateRangePicker` view, quick buttons)

- [ ] **Step 1: Add the button**

In `DateRangePicker`'s view, the quick list becomes:

```rust
            <div class="home-range-quick">
                <button on:click=move |_| last_hours(3)>"Last 3h"</button>
                <button on:click=today>"Today"</button>
                <button on:click=move |_| last_hours(24)>"Last 24h"</button>
                <button on:click=move |_| last_hours(24 * 7)>"Last 7 days"</button>
            </div>
```

- [ ] **Step 2: Check + commit**

Run: `just check`
Expected: clean.

```bash
git add crates/chaos-ui/src/pages/home.rs
git commit -m "feat(home): last-3h quick range"
```

---

### Task 4: Vendor ECharts + wasm-bindgen glue

**Files:**
- Create: `crates/chaos-web/vendor/echarts.min.js` (downloaded, pinned 5.6.0)
- Create: `crates/chaos-ui/src/echarts.rs`
- Modify: `crates/chaos-web/index.html`
- Modify: `Cargo.toml` (workspace deps), `crates/chaos-ui/Cargo.toml`
- Modify: `crates/chaos-ui/src/lib.rs` (module declaration)

- [ ] **Step 1: Download ECharts (pinned)**

```bash
mkdir -p crates/chaos-web/vendor
curl -sL https://cdn.jsdelivr.net/npm/echarts@5.6.0/dist/echarts.min.js \
  -o crates/chaos-web/vendor/echarts.min.js
head -c 100 crates/chaos-web/vendor/echarts.min.js   # sanity: JS, not an error page
```

Expected: file ~1 MB starting with a license banner/minified JS.

- [ ] **Step 2: Serve it from index.html**

`crates/chaos-web/index.html` becomes:

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>chaos</title>
    <link data-trunk rel="css" href="styles.css" />
    <link data-trunk rel="copy-dir" href="vendor" />
    <!-- Self-hosted ECharts (Home tab chart); pinned, no CDN. -->
    <script src="/vendor/echarts.min.js"></script>
  </head>
  <body></body>
</html>
```

- [ ] **Step 3: Add wasm-bindgen + serde_json to chaos-ui**

In the root `Cargo.toml` under `[workspace.dependencies]` add (version matches what leptos already locks, so the flake's pinned wasm-bindgen-cli stays valid):

```toml
wasm-bindgen = "0.2"
```

In `crates/chaos-ui/Cargo.toml` under `[dependencies]` add:

```toml
serde_json.workspace = true
wasm-bindgen.workspace = true
```

- [ ] **Step 4: Write the glue module**

Create `crates/chaos-ui/src/echarts.rs`:

```rust
//! Minimal bindings to the vendored Apache ECharts bundle (loaded globally
//! from index.html). Only the surface the Home tab chart uses — options are
//! passed as JSON built with serde_json and parsed on the JS side.

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    pub type EChart;

    /// `echarts.init(el)` — one chart instance bound to a DOM element.
    #[wasm_bindgen(js_namespace = echarts, catch)]
    pub fn init(el: &web_sys::HtmlElement) -> Result<EChart, JsValue>;

    #[wasm_bindgen(method, js_name = setOption, catch)]
    pub fn set_option(this: &EChart, option: &JsValue) -> Result<(), JsValue>;

    #[wasm_bindgen(method, js_name = dispatchAction, catch)]
    pub fn dispatch_action(this: &EChart, action: &JsValue) -> Result<(), JsValue>;

    #[wasm_bindgen(method, catch)]
    pub fn resize(this: &EChart) -> Result<(), JsValue>;

    #[wasm_bindgen(method, catch)]
    pub fn dispose(this: &EChart) -> Result<(), JsValue>;
}

/// Parse a JSON string into a JS object (NULL on bad input — callers treat
/// every interop step as fallible, the chart just stays empty).
pub fn json(raw: &str) -> JsValue {
    js_sys::JSON::parse(raw).unwrap_or(JsValue::NULL)
}
```

In `crates/chaos-ui/src/lib.rs`, next to `mod components;`:

```rust
mod components;
mod echarts;
mod pages;
```

- [ ] **Step 5: Check + commit**

Run: `just check`
Expected: clean (dead-code warnings are acceptable only if clippy passes; the module is consumed in the next task — if `-D warnings` trips on unused items, add `#[allow(dead_code)]` on the module line and remove it in Task 5).

```bash
git add crates/chaos-web/vendor crates/chaos-web/index.html Cargo.toml Cargo.lock \
        crates/chaos-ui/Cargo.toml crates/chaos-ui/src/echarts.rs crates/chaos-ui/src/lib.rs
git commit -m "feat(ui): vendor echarts 5.6.0 with minimal wasm-bindgen glue"
```

---

### Task 5: TemperatureChart on ECharts

**Files:**
- Modify: `crates/chaos-ui/src/pages/home.rs` (replace `TemperatureChart` and delete the hand-rolled SVG code)
- Modify: `crates/chaos-web/styles.css` (replace the `.temp-chart*` blocks)

- [ ] **Step 1: Replace the component**

In `crates/chaos-ui/src/pages/home.rs`, replace the whole `TemperatureChart` component (the SVG version, its `lines` computation and legend markup — keep `SERIES_COLORS`) with:

```rust
/// Multi-room temperature history on ECharts (vendored, see
/// chaos-ui/src/echarts.rs): hover tooltip with every visible room's value,
/// click the legend to hide a room (the y-axis stays fixed — it is pinned
/// from all series), drag a horizontal span to zoom (client-side; HA
/// history is already full resolution for the fetched window).
#[component]
fn TemperatureChart(
    series: Vec<TemperatureSeries>,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> impl IntoView {
    if !series.iter().any(|s| !s.readings.is_empty()) {
        return view! { <p class="muted">"No readings in this range."</p> }.into_any();
    }

    let node = NodeRef::<leptos::html::Div>::new();
    let chart = StoredValue::new_local(None::<crate::echarts::EChart>);

    Effect::new(move |_| {
        let Some(el) = node.get() else {
            return;
        };
        let instance = match chart.get_value() {
            Some(instance) => instance,
            None => match crate::echarts::init(&el) {
                Ok(instance) => {
                    chart.set_value(Some(instance.clone()));
                    instance
                }
                // Bundle missing/init failed: leave the div empty rather
                // than panic; the page still works.
                Err(_) => return,
            },
        };
        let option = crate::echarts::json(&chart_option(&series, start, end).to_string());
        let _ = instance.set_option(&option);
        // Fresh data ⇒ fresh window; and keep the drag-select zoom armed
        // (it is a toolbox feature, armed programmatically so no toolbox
        // icon has to be clicked — the toolbox itself stays hidden).
        let _ = instance.dispatch_action(&crate::echarts::json(
            r#"{"type":"dataZoom","start":0,"end":100}"#,
        ));
        let _ = instance.dispatch_action(&crate::echarts::json(
            r#"{"type":"takeGlobalCursor","key":"dataZoomSelect","dataZoomSelectActive":true}"#,
        ));
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

    view! { <div class="temp-chart" node_ref=node></div> }.into_any()
}

/// The ECharts option for the fetched series, themed from the CSS palette.
fn chart_option(
    series: &[TemperatureSeries],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> serde_json::Value {
    let fahrenheit = crate::weather_fahrenheit();
    let unit = if fahrenheit { "°F" } else { "°C" };
    let convert = move |celsius: f64| {
        let value = if fahrenheit {
            celsius * 9.0 / 5.0 + 32.0
        } else {
            celsius
        };
        // One decimal: these values land verbatim in the tooltip.
        (value * 10.0).round() / 10.0
    };

    // Y-scale pinned from ALL series so hiding a room never rescales.
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for reading in series.iter().flat_map(|s| &s.readings) {
        let value = convert(reading.celsius);
        min = min.min(value);
        max = max.max(value);
    }
    if !min.is_finite() {
        (min, max) = (0.0, 1.0);
    }
    let (min, max) = ((min - 0.5).floor(), (max + 0.5).ceil());

    let text = css_var("--text");
    let muted = css_var("--muted");
    let border = css_var("--border");
    let surface = css_var("--surface");

    let series_json: Vec<serde_json::Value> = series
        .iter()
        .enumerate()
        .map(|(i, s)| {
            serde_json::json!({
                "name": s.label,
                "type": "line",
                "showSymbol": false,
                "color": SERIES_COLORS[i % SERIES_COLORS.len()],
                "lineStyle": { "width": 1.5 },
                "data": s
                    .readings
                    .iter()
                    .map(|r| serde_json::json!([r.at.timestamp_millis(), convert(r.celsius)]))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();

    serde_json::json!({
        "animation": false,
        "grid": { "left": 44, "right": 16, "top": 36, "bottom": 28 },
        "legend": { "top": 0, "textStyle": { "color": text }, "inactiveColor": muted },
        "tooltip": {
            "trigger": "axis",
            "backgroundColor": surface,
            "borderColor": border,
            "textStyle": { "color": text },
        },
        // Hidden toolbox: only its dataZoom feature exists, armed from
        // TemperatureChart via takeGlobalCursor for direct drag-zoom.
        "toolbox": { "show": false, "feature": { "dataZoom": { "yAxisIndex": "none" } } },
        "xAxis": {
            "type": "time",
            "min": start.timestamp_millis(),
            "max": end.timestamp_millis(),
            "axisLabel": { "color": muted },
            "axisLine": { "lineStyle": { "color": border } },
            "splitLine": { "show": false },
        },
        "yAxis": {
            "type": "value",
            "min": min,
            "max": max,
            "axisLabel": { "color": muted, "formatter": format!("{{value}}{unit}") },
            "splitLine": { "lineStyle": { "color": border } },
        },
        "series": series_json,
    })
}

/// A CSS custom property from the active theme (empty string if unset —
/// ECharts then falls back to its defaults, which is survivable).
fn css_var(name: &str) -> String {
    web_sys::window()
        .and_then(|w| {
            let body = w.document()?.body()?;
            w.get_computed_style(&body).ok().flatten()
        })
        .and_then(|style| style.get_property_value(name).ok())
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}
```

Also delete from `home.rs`: nothing else — `SERIES_COLORS` stays; the old `lines`/legend/min-max code lived inside the replaced component. Ensure `use leptos::prelude::*;` already covers `NodeRef`, `StoredValue`, `Effect`, `window_event_listener`, `on_cleanup` (it does — they are all in the prelude).

Check whether `get_computed_style` needs the `CssStyleDeclaration` web-sys feature: in `crates/chaos-ui/Cargo.toml`, add `"CssStyleDeclaration"` to the web-sys features list.

- [ ] **Step 2: Replace the chart CSS**

In `crates/chaos-web/styles.css`, replace all `.temp-chart*` rules (container, `-plot`, `-yaxis`, `-svg`, `-line`, `-legend`, `-legend-item`, `-swatch`, `-range`) with:

```css
/* Home temperature chart (ECharts renders into this div). */
.temp-chart {
  height: 320px;
}

@media (max-width: 40rem) {
  .temp-chart {
    height: 240px;
  }
}
```

- [ ] **Step 3: Compile check**

Run: `just check`
Expected: clean, wasm target included. Remove any temporary `#[allow(dead_code)]` left on `mod echarts` in Task 4.

- [ ] **Step 4: Commit**

```bash
git add crates/chaos-ui crates/chaos-web/styles.css
git commit -m "feat(home): interactive temperature chart on echarts"
```

---

### Task 6: End-to-end verification (stub HA + screenshots)

The chart needs data; production HA's token is unreadable here, so a stub HA feeds a scratch chaos-server (established e2e technique: delay-image injection + headless Firefox).

**Files:** scratchpad only (nothing committed).

- [ ] **Step 1: Build the frontend and a stub HA**

```bash
cd crates/chaos-web && nix develop -c trunk build && cd ../..
S=$CLAUDE_JOB_DIR/tmp/e2e-home; rm -rf $S && mkdir -p $S/dist
cp -r crates/chaos-web/dist/* $S/dist/
sed -i 's|</body>|<img src="https://httpbin.org/delay/8" style="display:none"></body>|' $S/dist/index.html
cat > $S/stub-ha.js <<'EOF'
const http = require("http");
const now = Date.now();
const series = (base) => Array.from({length: 48}, (_, i) => ({
  state: (base + 2*Math.sin(i/6) + Math.random()*0.4).toFixed(1),
  last_changed: new Date(now - (48 - i) * 30 * 60 * 1000).toISOString(),
}));
http.createServer((req, res) => {
  res.setHeader("content-type", "application/json");
  if (req.url.startsWith("/api/history/period")) {
    res.end(JSON.stringify([series(21), series(19), series(17.5)]));
  } else if (req.url.startsWith("/api/states/")) {
    res.end(JSON.stringify({state: "on", attributes: {brightness: 128}}));
  } else if (req.url.startsWith("/api/template")) {
    res.end("Salon");
  } else if (req.method === "POST") {
    res.end("[]");
  } else { res.statusCode = 404; res.end("{}"); }
}).listen(65030);
EOF
echo stub-token > $S/token
nix-shell -p nodejs_22 --run "node $S/stub-ha.js" &
```

- [ ] **Step 2: Scratch chaos.toml with three sensors + a light**

```bash
S=$CLAUDE_JOB_DIR/tmp/e2e-home
cat > $S/chaos.toml <<EOF
static_dir = "$S/dist"
listen = "127.0.0.1:4699"
database_url = "sqlite://$S/chaos.db?mode=rwc"

[home_assistant]
base_url = "http://127.0.0.1:65030"
token_file = "$S/token"

[[home_assistant.sensors]]
id = "salon"
label = "Salon"
entity_id = "sensor.salon"

[[home_assistant.sensors]]
id = "chambre"
label = "Chambre"
entity_id = "sensor.chambre"

[[home_assistant.sensors]]
id = "bureau"
label = "Bureau"
entity_id = "sensor.bureau"

[[home_assistant.lights]]
id = "kajplats"
label = "Kajplats"
entity_id = "light.kajplats"
EOF
cargo build -p chaos-server -q
CHAOS_CONFIG=$S/chaos.toml /projects/rust/chaos/target/debug/chaos-server > $S/server.log 2>&1 &
sleep 2 && curl -s http://127.0.0.1:4699/api/v1/home/temperature?start=$(date -u -d '-24 hours' +%FT%TZ)\&end=$(date -u +%FT%TZ) | head -c 200
```

Expected: JSON with three series and readings.

- [ ] **Step 3: Screenshots (desktop + phone)**

```bash
S=$CLAUDE_JOB_DIR/tmp/e2e-home
firefox --headless --no-remote --profile $(mktemp -d) --window-size=1440,1000 \
  --screenshot $S/home-desktop.png http://127.0.0.1:4699/home 2>/dev/null
firefox --headless --no-remote --profile $(mktemp -d) --window-size=390,900 \
  --screenshot $S/home-phone.png http://127.0.0.1:4699/home 2>/dev/null
```

Read both images and verify: chart canvas rendered with 3 colored lines + top legend + axis labels with ° unit; light card contains the slider inside its border at both widths (the 390px shot is the overflow regression check). Hover/drag can't be screenshotted headlessly — verified live after deploy.

- [ ] **Step 4: Light toggle against the stub**

```bash
curl -s -X POST http://127.0.0.1:4699/api/v1/home/lights/kajplats \
  -H 'content-type: application/json' -d '{"on": true}'
```

Expected: `"on":true` in the response (stub always says on, so it confirms on the first poll). Kill the scratch processes afterwards (`pkill -x chaos-server -u $(id -u)`; kill the node stub).

- [ ] **Step 5: Final gate + push + PR**

```bash
just check && cargo test --workspace
git push -u origin feat/home-chart-interactivity
gh pr create --base develop \
  --title "feat(home): interactive temperature chart, light control fixes" \
  --body "Implements docs/superpowers/specs/2026-07-08-home-tab-chart-and-lights-design.md: ECharts chart (hover tooltip, legend toggle, drag zoom, Last 3h), server-confirmed light commands (no more self-unchecking checkbox), contained brightness slider."
```

Expected: PR opens against develop; CI (`check` + `build`) goes green.

---

## Self-review notes

- Spec coverage: tooltip/legend/fixed-y/drag-zoom/Last-3h → Tasks 3-5; checkbox → Tasks 1-2; slider → Task 2; error handling (init failure, poll errors) → Tasks 1 & 5; testing → Tasks 1 & 6. Touch gestures explicitly out of scope.
- Tooltip values are pre-rounded to one decimal and the ° unit lives on the y-axis labels — no JS formatter closures needed (deliberate simplification vs. the spec's "tooltip with °C/°F formatting"; the unit is visible on the axis and the values honor the preference).
- `dataZoom` reset uses `dispatchAction` after every `setOption` so a refetch (range change) always shows the full new window.
