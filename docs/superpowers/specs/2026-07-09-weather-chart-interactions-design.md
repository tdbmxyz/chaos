# Weather Chart Interactions — Design

**Date:** 2026-07-09

Follow-up to `2026-07-09-weather-hourly-chart-design.md` (the Weather hourly ECharts
line chart). Three refinements:

1. **Gradual zoom-out** — double left-click (desktop) steps the visible window out;
   pinch-out (touch) zooms out progressively. Applies to the Home chart too.
2. **Combined hover tooltip** — hovering any weather chart shows one tooltip listing
   *every* location's temperature for that hour, for easy comparison.
3. **Shared y-axis scale** — every weather chart uses the same fixed y-range, computed
   from the min/max over *all* locations' data points. The range does not rescale
   while zooming/panning.

## Architecture: all locations' series in every chart

Items 2 and 3 both need each chart to know every location's data. Chosen approach:
each weather chart's ECharts option includes **all locations as series** — its own
location drawn normally (accent color), the others with an **invisible line**
(`lineStyle.opacity: 0`) but a real per-location color. ECharts' built-in axis
tooltip then lists every series (name + colored marker + value) with zero custom
formatter — important because the JSON option bridge cannot carry JS functions.

Rejected alternatives: a custom JS tooltip formatter (needs new wasm↔JS closure
machinery inside `setOption`, untestable natively, and #3 needs shared data anyway);
a single merged chart (changes the one-row-per-location page layout).

### Shared hourly-data signal (`weather.rs`)

- `WeatherPage` owns `RwSignal<Vec<(String, Vec<HourlyForecast>)>>` — one entry per
  *loaded* location, keyed by the location's display name from the API response,
  insertion-ordered as fetches resolve.
- Each `WeatherRow` receives the signal. When its `LocalResource` resolves Ok with a
  non-empty `hourly`, an `Effect` upserts `(location_name, hourly)` into the signal
  (upsert, not push: refetches must not duplicate).
- Removing a location removes its entry (retain by the *place strings* the page
  tracks is not possible — entries are keyed by resolved display name — so the row's
  cleanup (`on_cleanup`) removes its own entry by name).
- The default row (no configured places) participates identically; with a single
  entry the "combined" tooltip and "shared" scale degrade to the current behavior.

### Chart option (`weather_chart_option`)

Signature grows to take the full collection and the identity of the owning row:

```rust
fn weather_chart_option(
    own: &str,                                      // this row's location name
    all: &[(String, Vec<HourlyForecast>)],          // every loaded location
    fahrenheit: bool,
    colors: &ChartColors,
) -> serde_json::Value
```

- **x-axis**: unchanged — the *own* location's emoji/hour labels, category axis,
  `interval: 2`.
- **Series**: one per entry in `all`, `name` = location name, aligned by hour index
  (same equal-length assumption as the existing index-based zoom sync; a shorter
  series just ends early). Every series gets a color from a small fixed palette,
  cycled by its index in `all` — so a location's tooltip marker color is stable
  across charts — except the owning chart's *own* series, which uses the accent
  color and stays visible (width 1.5). All other series: `lineStyle: { opacity: 0 }`
  and `silent: true` so invisible lines don't catch events.
- **y-axis**: explicit `min`/`max` replacing `scale: true`. Computed in Rust:
  global min/max over every location's converted temps, padded by 1 display degree
  and rounded outward to whole degrees. Fixed under zoom (dataZoom's default
  `filterMode` y-rescaling is overridden by the explicit bounds). Identical inputs
  on every chart ⇒ identical range.
- **Tooltip**: unchanged config (`trigger: "axis"`); it now naturally lists all
  locations. Order = `all` order, stable across charts.
- Empty guard: a row renders its chart as soon as *its own* data is loaded; other
  locations appear in its tooltip as they arrive (option builder re-runs reactively
  via the signal).

## Gradual zoom-out (`echarts.rs`, shared by Home + Weather)

`ChartCanvas` arms a double-click handler on every chart:

- New bindings: `getZr()` → zrender handle, `zr.on("dblclick", closure)`,
  `chart.get_option()` (to read the live dataZoom window), plus a
  `Closure<dyn FnMut()>` stored for the component's lifetime and dropped on cleanup.
- Handler: read current `dataZoom[0]` `start`/`end` percentages; compute the widened
  window with a pure function:

```rust
/// Widen a dataZoom window [start, end] (percent) by 2× around its center,
/// clamped and shifted to stay within [0, 100].
fn widen_window(start: f64, end: f64) -> (f64, f64)
```

  (full range in → full range out; degenerate `start == end` widens to a minimum
  span of 5%). Dispatch `{"type": "dataZoom", "start": s, "end": e}`.
- Connect-group propagation makes all weather charts step out together; the Home
  chart (no group) steps out alone.
- **Pinch-out on touch**: already provided by the `inside` dataZoom (pinch is native
  to it); verified manually, no code change.

## Testing

Native unit tests (`cargo test -p chaos-ui`):

- `widen_window`: doubles around center, clamps at edges, full-range fixed point,
  minimum span.
- y-range computation: min/max across multiple locations, °F conversion, padding
  and outward rounding.
- `weather_chart_option`: N series for N locations, exactly one visible (others
  `opacity: 0`), series names = location names, explicit `yAxis.min`/`max`, own
  location's labels on the x-axis.

Interop (bindings, closure lifetime, group propagation of the dataZoom action)
via `just check` (wasm compile) and manual testing in the browser.

## Cleanup

- `hourly_temps`/`hourly_labels` stay; the option builder maps them over `all`.
- No CSS changes.
