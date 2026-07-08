# Home tab: interactive temperature chart + light control fixes

2026-07-08 · validated with Tibo (brainstorming session)

## Problem

1. The temperature history chart is hard to read and static: no way to see
   values, hide a room, or zoom into a time span.
2. The light card's checkbox unchecks itself right after being checked even
   though the light turns on: the server re-fetches HA state immediately
   after `turn_on`, and HA often still reports `off` for a second or two
   (Zigbee confirms asynchronously), so the stale response overwrites the
   fresh check.
3. The brightness slider overflows the light card: `input[type=range]` has
   an intrinsic ~130px minimum width that `flex: 1` alone cannot shrink.

## Decisions (from brainstorming)

- Hover behavior: floating tooltip at the cursor with a vertical guide line,
  all visible rooms' values at that time (option A).
- Legend: click a room to hide/show its line; hidden entries render dimmed
  and struck through.
- Y-axis stays **fixed** when hiding lines (computed from all series).
- Zoom: click+drag a horizontal span to zoom into that time range; a new
  **Last 3h** quick-range button joins Today / Last 24h / Last 7 days.
- Implementation: a charting library rather than extending the hand-rolled
  SVG — Apache **ECharts**, vendored (no CDN), driven through a thin
  hand-written wasm-bindgen glue (not the charming crate).
- Lights: **optimistic UI + server confirmation** (approach 3): the card
  flips immediately, and the server polls HA until the commanded state is
  observed before answering, so the response is never stale.

## Design

### Chart (ECharts)

- `crates/chaos-web` vendors a pinned `echarts.min.js`, copied into the dist
  by trunk and loaded from `index.html`. Self-hosted: works offline and
  inside the Tauri/Android shells.
- New `crates/chaos-ui/src/echarts.rs`: minimal `wasm_bindgen` bindings —
  `init(element)`, `setOption(json)`, `resize()`, `dispose()`, and `on()`
  for the few events we need. Only the API surface we use.
- `TemperatureChart` (pages/home.rs) becomes a `<div node_ref>` plus an
  `Effect` that serializes an option object (serde_json) from the fetched
  `Vec<TemperatureSeries>` and calls `setOption`.
- Option mapping:
  - `tooltip: { trigger: "axis" }` with a formatter applying the °C/°F
    device preference (`crate::weather_fahrenheit()`); axis pointer = the
    vertical guide line.
  - `legend` with default `selectedMode` (click toggles a series).
  - `yAxis.min`/`yAxis.max` pinned from **all** series so toggling never
    rescales.
  - `dataZoom` brush (`toolbox.feature.dataZoom`, `xAxis` only): drag a
    span to zoom. The brush is armed permanently at init with
    `dispatchAction({type: "takeGlobalCursor", key: "dataZoomSelect",
    dataZoomSelectActive: true})` so plain click+drag zooms without first
    clicking a toolbox icon (the toolbox itself stays hidden). Zoom is
    client-side — HA history is already delivered at full resolution for
    the fetched window, so nothing is lost. Quick-range buttons reset it by
    refetching.
  - Series colors: the existing `SERIES_COLORS`; text/axis/grid colors read
    from the CSS custom properties (`--text`, `--muted`, `--border`) at
    mount so every theme stays coherent.
- The date-range picker keeps its role (server-side window). New quick
  button: Last 3h.

### Lights: no stale state, optimistic feel

- Server (`home_assistant.rs::set_light`): after the service call, poll
  `fetch_light_state` every 250 ms for up to 2 s until the observed state
  matches the command (`on` for toggles; brightness applied for
  adjustments). Return the first match; on timeout return the last fetch —
  a genuinely failed light still reports the truth. Errors during
  confirmation polls are ignored (last good state wins); the initial
  command's error mapping (502) is unchanged. No API shape change.
- UI (`LightCard`): set the `on` signal optimistically on toggle (the
  native checkbox already flips; the signal follows so dependent state is
  coherent). `apply_state` keeps reconciling from the (now confirmed)
  response. If the server reports a real `off` after timeout, the checkbox
  reverts — correct, since the light didn't turn on.

### Slider overflow (CSS)

- `.light-card-row input[type="range"] { min-width: 0; }` (keeps `flex: 1`).
- Percentage readout: fixed `3.2rem`, right-aligned, `tabular-nums`, so
  "5%" vs "100%" doesn't shift the row.

## Error handling

- ECharts glue: all interop calls are checked; failure to load/init leaves
  the chart div empty with the existing `.error` paragraph fallback. No
  panics.
- HA unreachable: unchanged — temperature endpoint 502s (shown in the
  section), lights render `available: false` cards.

## Testing / verification

- `just check` + `cargo test --workspace` (unit test for the confirm-poll
  timeout path with a stubbed HA state sequence).
- Headless-Firefox screenshots against a scratch server for the chart
  render and light card layout at desktop and 390px widths.
- Live verification on zeus after deploy: hover tooltip, legend toggle,
  drag zoom, checkbox stays checked, slider contained.

## Out of scope

- Touch-specific chart gestures (pinch zoom); ECharts' defaults apply.
- Any HA entity configuration changes; label resolution shipped in 1.1.2.
