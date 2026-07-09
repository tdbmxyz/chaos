# Weather Hourly Forecast Chart — Design

**Date:** 2026-07-09

## Goal

Replace the Weather tab's horizontal hourly *strip* (`.hourly-strip`, one cell per
hour with time · emoji · temperature) with an ECharts line chart of the 48-hour
temperature forecast, matching the visual language of the Home tab's
`TemperatureChart`. The x-axis carries a two-line label — **weather emoji on top,
date/hour below** — and all the per-location weather charts are **synchronized**:
panning/zooming one pans/zooms them all.

## Scope

- Weather page only (`crates/chaos-ui/src/pages/weather.rs`). The current-conditions
  header (emoji · place · temp · feels/wind/humidity) is unchanged; only the hourly
  strip below it is replaced.
- Synchronization is between the Weather location charts **only**. The Home chart is a
  different time domain (user-selected history vs. 48 h forecast) and is deliberately
  left out of the group.

## Chart specification

- **Category x-axis**, one category per forecast hour. Chosen over a time axis so
  every tick can carry a custom two-line label, and so cross-location zoom syncs by
  forecast-hour *index* rather than wall-clock (each location reports its own local
  time via Open-Meteo `timezone=auto`, so "hour +6" should align, not "18:00").
- **Two-line axis label**: `weather_emoji(code)` on line 1; `"{hour}h"` on line 2,
  except at midnight (`time.hour() == 0`) where line 2 is the weekday (`%a`) — same
  rule the strip used. Labels render at **fixed cadence, every 3rd hour**
  (`axisLabel.interval = 2`). Zoom/pan stays enabled; the cadence is re-applied to
  whatever range is visible.
- **Single temperature line** in the accent color, values converted to °C/°F per
  `weather_fahrenheit()` and rounded to one decimal.
- **Tooltip** (`trigger: "axis"`): hour label + temperature.
- **Zoom/pan**: drag-select zoom + inside pinch/pan, reusing the Home chart's
  transparent-off-canvas toolbox + `takeGlobalCursor` arming.

## Synchronization

All weather charts join one ECharts *connect group* (constant `"weather"`). ECharts
then syncs dataZoom and the axis tooltip across every chart in the group. Needs two
new bindings in `echarts.rs`: a `group` setter and `echarts.connect(group)`. Charts
mount asynchronously (one `LocalResource` per location); each chart sets its group and
calls `connect("weather")` after `set_option`, which (re)connects the whole group as
members appear. Disposing a chart (page navigation / removing a location) drops it
from the group.

## Code structure

Home's `TemperatureChart` and the new weather chart share ~40 lines of identical
ECharts mount boilerplate (init → `set_option` → resize listener → dispose → drag-zoom
arming). Extract a reusable **`ChartCanvas`** component into `echarts.rs`:

- Props: `option: Callback<(), serde_json::Value>` (reactive option builder),
  `group: Option<&'static str>` (connect group), `class: &'static str` (sizing class).
- Handles init, `set_option`, drag-zoom arming, resize, dispose, and (if `group` is
  set) group assignment + `connect`.

`TemperatureChart` migrates onto `ChartCanvas` (keeping its own option-builder). The
per-chart ECharts *option* JSON stays local to each page.

`css_var` (currently private in `home.rs`) moves to `echarts.rs` as `pub(crate)` so
both pages read theme colors from one place. Theme colors are read via `css_var`
(DOM-reading, browser-only — it *panics* off-wasm, since wasm-bindgen imports can't run
natively) into a `ChartColors` struct, which is **injected** into a pure option builder.
The label / temperature logic is then unit-testable natively with a default (empty)
`ChartColors`.

## Testing

Pure, native-testable units in `weather.rs`:

- `hourly_labels(&[HourlyForecast]) -> Vec<String>` — `"emoji\n{hour}h"`, or
  `"emoji\n{weekday}"` at midnight.
- `hourly_temps(&[HourlyForecast], fahrenheit: bool) -> Vec<f64>` — converted,
  one-decimal.

These are exercised by `cargo nextest run -p chaos-ui`. Interop (ECharts bindings,
`ChartCanvas`, connect) is verified by `just check` (wasm compile) and manually in the
running app.

## Cleanup

- Remove `.hourly-strip`, `.hour-cell`, `.hour-label` CSS rules; add `.weather-chart`
  (height, mirroring `.temp-chart`).
- `chrono::Timelike` stays (still used for the midnight check).
