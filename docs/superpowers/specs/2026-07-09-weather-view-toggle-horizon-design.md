# Weather View Toggle, Zoom UX, and ±16-Day Horizon — Design

**Date:** 2026-07-09

Third iteration on the Weather charts (after `2026-07-09-weather-hourly-chart-design.md`
and `2026-07-09-weather-chart-interactions-design.md`). User feedback on the last round:

1. The invisible-sibling-series combined tooltip is not the right UX. Instead: a
   **toggle button** switches the page between *split* view (one chart per location,
   as originally) and *combined* view (one chart with every location's line).
2. The gradual double-click zoom-out feels clunky. Replace with **wheel/pinch zoom +
   drag pan + double-click reset**.
3. 48 h is too restrictive. Extend to **16 days past + 16 days forecast**; charts
   open on a default window of **past 24 h + next 48 h**.

## 1. Server: ±16-day horizon (`chaos-server`, `chaos-domain`)

- Open-Meteo forecast URL gains `&past_days=16` and changes `forecast_days=5` → `16`
  (one request still covers past + future; `past_days` returns observed/archived
  values seamlessly in the same hourly arrays).
- The current-hour truncation (`.filter(|h| h.time >= this_hour).take(48)`) is
  **removed**: `hourly` now carries the full ~768 location-local points.
- `WeatherData` (in `chaos-domain`) gains `pub now_index: usize` — the index of the
  first hourly entry at or after the location-local current hour (`this_hour`, the
  same floor-to-hour anchor the server computes today). If no entry qualifies
  (cannot happen with a 16-day forecast, but keep it total), `now_index` is
  `hourly.len()`.
- No other consumer reads `hourly` (verified: the dashboard widget uses only
  current conditions + `daily`), so nothing else changes. Payload grows to ~45 KB
  per location — acceptable.
- Unit test: `now_index` finding (mid-series, exact-hour match, and the
  all-in-the-past degenerate case).

## 2. Zoom UX (shared `ChartCanvas`, both pages)

- The `inside` dataZoom becomes fully interactive: `zoomOnMouseWheel: true` (zoom
  around the cursor), `moveOnMouseMove: true` (drag pans), touch pinch as before.
  Set in each option builder (Home + Weather).
- The drag-select zoom is **removed**: drop the transparent off-canvas `toolbox`
  block from both option builders and the `takeGlobalCursor` arming dispatch from
  `ChartCanvas` — drag now means pan.
- Double-click **resets** to the default window. `ChartCanvas` gets a new prop
  `reset_zoom: (f64, f64)` (percent of full range, default `(0.0, 100.0)`); the
  dblclick handler dispatches `{"type": "dataZoom", "start": s, "end": e}` with
  those values. The stepping machinery — `widen_window`, `zoom_window`, and the
  `getOption` binding — is **deleted** (the `getZr`/`on` bindings stay for the
  handler itself).
- Weather charts stay in the `"weather"` connect group, so wheel-zoom, pan, and
  the dblclick reset all stay synchronized across locations; Home is ungrouped.

## 3. Default window & now-marker

- A pure helper computes the default window in percent:
  `default_window(now_index, len) -> (f64, f64)` = hours `[now_index − 24,
  now_index + 48]` as percentages of `len`, clamped to `[0, 100]` (and `(0, 100)`
  for degenerate `len`). Weather pages pass it as `ChartCanvas`'s `reset_zoom`;
  Home passes nothing (full range).
- The option JSON **never** carries `dataZoom.start`/`end`. Instead `ChartCanvas`
  dispatches `{"type": "dataZoom", "start": s, "end": e}` with `reset_zoom` once,
  right after the FIRST `set_option` of the mount (and again on every dblclick).
  Rationale: options re-set reactively as sibling forecasts stream in; a merged
  dataZoom carrying `start`/`end` would snap a user-adjusted window back to the
  default on every re-render, while a start/end-less dataZoom merge preserves the
  live window. Side effect (intended): the initial dispatch propagates through the
  `"weather"` connect group, so adding a location realigns all synced charts to
  the default window — without it the new chart and its zoomed siblings would
  disagree.
- A vertical **now-marker** separates past from forecast: a `markLine` on the own
  series (split view) / first series (combined view) at x-axis index `now_index`,
  `silent`, muted color, no symbol, small "now" label.
- Because `set_option` re-runs reactively with a dataZoom that carries
  `start`/`end`, sibling stream-ins would snap a user's pan/zoom back to the
  default. To avoid that, the option builder emits `start`/`end` only via the same
  JSON, and dataZoom updates merge — verify during implementation that a re-set
  option with identical `start`/`end` does not reset a user-adjusted window; if it
  does, drop `start`/`end` from re-renders (e.g. include them only on the first
  build per mount) — the plan must make this concrete.

## 4. Axis labels at 32-day scale

- Two-line emoji labels stay, but the midnight second line becomes `%a %-d`
  (e.g. `Fri 12`) so days are identifiable across weeks. Non-midnight stays
  `{hour}h`.
- `axisLabel.interval: 2` (fixed cadence) is replaced by ECharts auto-thinning:
  omit `interval` and set `hideOverlap: true`, so label density adapts to the
  zoom level.
- Combined chart labels: same rhythm minus the emoji line (emoji are
  per-location), built from the **first** loaded location's timestamps. The
  hour-index alignment caveat across timezones (documented in the previous spec)
  carries over.

## 5. Split / combined view toggle (`weather.rs` page)

- `WeatherPage` owns `combined: RwSignal<bool>` (session-only, starts `false`)
  and renders a toggle button in the page header (next to the Add form):
  labelled "Combine" in split mode, "Split" in combined mode.
- **Split mode** (default): one row per location exactly as before, but
  `weather_chart_option` **reverts to a single own series** — the invisible
  sibling-series mechanism is deleted. Tooltip shows only the own location. The
  shared fixed `y_range` (over every loaded location) and the `"weather"`
  connect-group sync are kept.
- **Combined mode**: the compact current-conditions headers (emoji · place ·
  temp · details, with the remove button) render as a list — each row keeps its
  header but drops its chart — followed by ONE `ChartCanvas` (class
  `combined-chart`, no group) whose option comes from a new
  `combined_chart_option(all: &LoadedForecasts, fahrenheit, colors) ->
  serde_json::Value`: one visible line per location colored by
  `SIBLING_PALETTE[i % 6]` (the palette is repurposed — every line is visible
  here), an ECharts `legend` (location names), native multi-series axis tooltip,
  the same pinned `y_range`, the same default window/reset, and the now-marker.
- The `loaded` signal (page-wide `LoadedForecasts`) already exists and feeds the
  combined chart; rows keep publishing/retracting exactly as today. Empty
  `loaded` in combined mode shows the muted "Loading forecast…"-style empty state
  instead of an empty chart.
- The rows' `LocalResource` fetches are keyed to the row components; toggling
  modes must NOT unmount the rows (or forecasts would refetch on every toggle).
  Structure: rows always render; in combined mode each row renders header-only
  and the page appends the combined chart.

## 6. Cleanup

- Delete: invisible-sibling series logic and its tests (`option_embeds_all…`,
  `option_prepends_own…`, `sibling_marker_colors…`), `widen_window` + its 6
  tests, `zoom_window`, the `get_option` binding, both toolbox blocks, the
  `takeGlobalCursor` dispatch.
- Keep: `LoadedForecasts` signal + publish/retract, `y_range`, `SIBLING_PALETTE`
  (renamed `LOCATION_PALETTE` since it now colors real lines), `hourly_temps`,
  `hourly_labels` (label change per §4), `replaceMerge: ["series"]` (still needed
  — combined chart series count changes as locations load/unload).
- CSS: add `.combined-chart` (taller than `.weather-chart`, e.g. 320 px) and a
  header style for chart-less rows if needed; add toggle-button styling reusing
  the existing button look.

## 7. Testing

Pure/native (`cargo test -p chaos-ui` / `-p chaos-server`):

- `default_window`: centered case, clamp at series start (now_index < 24),
  clamp at end, degenerate empty series.
- `hourly_labels`: midnight now yields `"{emoji}\nFri 12"` form; combined-label
  variant has no emoji line.
- `weather_chart_option` (reverted): exactly one series, no legend, pinned
  y-range, dataZoom carries wheel/pan flags + default start/end, markLine at
  `now_index`.
- `combined_chart_option`: N visible series with palette colors + names, legend
  present, markLine on first series, pinned y-range.
- Server `now_index` (see §1).

Manual: zoom/pan/reset feel on desktop + phone, toggle round-trip without
refetch, sync in split mode, legend/tooltip in combined mode, now-marker
placement, label density at full zoom-out.
