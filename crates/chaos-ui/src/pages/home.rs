//! The Home tab: temperature history from Home Assistant sensors (with a
//! date/time range picker), light control (on/off, brightness, color), and
//! a sensor battery card summarizing low-battery devices.
//! Empty when the server has no `home_assistant` configured.

use chaos_domain::{LightCommand, LightState, RgbColor, TemperatureQuery, TemperatureSeries};
use chrono::{DateTime, Duration, Local, NaiveDateTime, TimeZone, Utc};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::use_client;

const SERIES_COLORS: [&str; 6] = [
    "#7c9aff", "#facc15", "#4ade80", "#f87171", "#c084fc", "#60d6e6",
];

#[component]
pub fn HomePage() -> impl IntoView {
    let client = use_client();

    let now = Utc::now();
    let range = RwSignal::new((now - Duration::days(7), now));

    let temperature = LocalResource::new({
        let client = client.clone();
        move || {
            let (start, end) = range.get();
            let client = client.clone();
            async move {
                client
                    .home_temperature(&TemperatureQuery { start, end })
                    .await
            }
        }
    });

    let lights = LocalResource::new({
        let client = client.clone();
        move || {
            let client = client.clone();
            async move { client.home_lights().await }
        }
    });

    let sensors = LocalResource::new({
        let client = client.clone();
        move || {
            let client = client.clone();
            async move { client.home_sensors().await }
        }
    });

    view! {
        <div class="home-page">
            <h2>"Home"</h2>

            <section class="home-section">
                <h3>"Temperature"</h3>
                <DateRangePicker range/>
                {move || match temperature.get() {
                    None => view! { <p class="muted">"Loading temperature history…"</p> }.into_any(),
                    Some(Err(err)) => view! { <p class="error">{err.to_string()}</p> }.into_any(),
                    Some(Ok(series)) if series.is_empty() => {
                        view! { <p class="muted">"No sensors configured."</p> }.into_any()
                    }
                    Some(Ok(series)) => {
                        let (start, end) = range.get();
                        view! { <TemperatureChart series start end/> }.into_any()
                    }
                }}
            </section>

            <section class="home-section">
                <h3>"Lights"</h3>
                {move || match lights.get() {
                    None => view! { <p class="muted">"Loading lights…"</p> }.into_any(),
                    Some(Err(err)) => view! { <p class="error">{err.to_string()}</p> }.into_any(),
                    Some(Ok(list)) if list.is_empty() => {
                        view! { <p class="muted">"No lights configured."</p> }.into_any()
                    }
                    Some(Ok(list)) => {
                        view! {
                            <div class="light-grid">
                                {list.into_iter().map(|light| view! { <LightCard light/> }).collect_view()}
                            </div>
                        }
                            .into_any()
                    }
                }}
            </section>

            <section class="home-section">
                <h3>"Sensors"</h3>
                {move || match sensors.get() {
                    None => view! { <p class="muted">"Loading sensors…"</p> }.into_any(),
                    Some(Err(err)) => view! { <p class="error">{err.to_string()}</p> }.into_any(),
                    Some(Ok(list)) if list.is_empty() => {
                        view! { <p class="muted">"No sensors configured."</p> }.into_any()
                    }
                    Some(Ok(list)) => view! {
                        <div class="sensor-list">
                            {list.into_iter().map(|sensor| view! { <SensorRow sensor/> }).collect_view()}
                        </div>
                    }
                        .into_any(),
                }}
            </section>
        </div>
    }
}

/// One sensor row: label plus its device battery (bar + percentage). An
/// em dash when the sensor exposes no battery entity.
#[component]
fn SensorRow(sensor: chaos_domain::HomeSensorInfo) -> impl IntoView {
    view! {
        <div class="sensor-row">
            <span class="sensor-label">{sensor.label}</span>
            {match sensor.battery_pct {
                Some(pct) => {
                    let pct = pct.clamp(0.0, 100.0);
                    view! {
                        <span class="sensor-battery">
                            <span class="battery-bar" class:low=move || pct < 20.0>
                                <span
                                    class="battery-fill"
                                    style:width=format!("{pct:.0}%")
                                ></span>
                            </span>
                            <span class="muted battery-pct">{format!("{pct:.0}%")}</span>
                        </span>
                    }
                        .into_any()
                }
                None => view! { <span class="muted battery-pct">"—"</span> }.into_any(),
            }}
        </div>
    }
}

/// Quick-range buttons plus a custom start/end date+time form (local
/// timezone in the inputs, converted to UTC for the query).
#[component]
fn DateRangePicker(range: RwSignal<(DateTime<Utc>, DateTime<Utc>)>) -> impl IntoView {
    let start_date = RwSignal::new(String::new());
    let start_time = RwSignal::new(String::new());
    let end_date = RwSignal::new(String::new());
    let end_time = RwSignal::new(String::new());

    // Keep the custom fields in sync whenever the range changes, including
    // from the quick buttons, so "Apply" starts from the current range.
    Effect::new(move |_| {
        let (start, end) = range.get();
        let start = start.with_timezone(&Local);
        let end = end.with_timezone(&Local);
        start_date.set(start.format("%Y-%m-%d").to_string());
        start_time.set(start.format("%H:%M").to_string());
        end_date.set(end.format("%Y-%m-%d").to_string());
        end_time.set(end.format("%H:%M").to_string());
    });

    let last_hours = move |hours: i64| {
        let end = Utc::now();
        range.set((end - Duration::hours(hours), end));
    };
    let today = move |_| {
        let end = Utc::now();
        let midnight = Local::now().date_naive().and_hms_opt(0, 0, 0).unwrap();
        let start = Local
            .from_local_datetime(&midnight)
            .single()
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or(end);
        range.set((start, end));
    };
    let apply = move |_| {
        let parse = |date: String, time: String| -> Option<DateTime<Utc>> {
            let naive =
                NaiveDateTime::parse_from_str(&format!("{date} {time}"), "%Y-%m-%d %H:%M").ok()?;
            Local
                .from_local_datetime(&naive)
                .single()
                .map(|dt| dt.with_timezone(&Utc))
        };
        if let (Some(start), Some(end)) = (
            parse(start_date.get(), start_time.get()),
            parse(end_date.get(), end_time.get()),
        ) && start < end
        {
            range.set((start, end));
        }
    };

    view! {
        <div class="home-range-picker">
            <div class="home-range-quick">
                <button on:click=move |_| last_hours(3)>"Last 3h"</button>
                <button on:click=today>"Today"</button>
                <button on:click=move |_| last_hours(24)>"Last 24h"</button>
                <button on:click=move |_| last_hours(24 * 7)>"Last 7 days"</button>
            </div>
            <div class="home-range-custom">
                <input
                    type="date"
                    prop:value=start_date
                    on:input=move |ev| start_date.set(event_target_value(&ev))
                />
                <input
                    type="time"
                    prop:value=start_time
                    on:input=move |ev| start_time.set(event_target_value(&ev))
                />
                <span class="muted">"to"</span>
                <input
                    type="date"
                    prop:value=end_date
                    on:input=move |ev| end_date.set(event_target_value(&ev))
                />
                <input
                    type="time"
                    prop:value=end_time
                    on:input=move |ev| end_time.set(event_target_value(&ev))
                />
                <button on:click=apply>"Apply"</button>
            </div>
        </div>
    }
}

/// Multi-room temperature history on ECharts (vendored, see
/// chaos-ui/src/echarts.rs): hover tooltip with every visible room's value,
/// click the legend to hide a room (the y-axis stays fixed — it is pinned
/// from all series), wheel to zoom around the cursor, drag to pan, pinch on
/// touch, double-click to reset to the full range (all client-side; HA
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

    let option = Callback::new(move |()| chart_option(&series, start, end));
    let reset = today_window(start, end);
    view! {
        <crate::echarts::ChartCanvas
            option
            reset_zoom=reset
            tooltip_formatter="chaosTimeTooltip"
            class="temp-chart"
        />
    }
    .into_any()
}

/// Percent window of `[start, end]` covering the current local day — the
/// chart's initial and double-click zoom. Falls back to the full range when
/// today's midnight precedes `start` (short custom ranges).
fn today_window(start: DateTime<Utc>, end: DateTime<Utc>) -> (f64, f64) {
    let midnight = Local::now().date_naive().and_hms_opt(0, 0, 0).unwrap();
    let Some(midnight) = Local.from_local_datetime(&midnight).single() else {
        return (0.0, 100.0);
    };
    today_window_at(midnight.with_timezone(&Utc), start, end)
}

/// Pure percent-window math for `today_window`, split out so it can be unit
/// tested without `Local::now()`.
fn today_window_at(
    midnight: DateTime<Utc>,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> (f64, f64) {
    let span = (end - start).num_milliseconds() as f64;
    // Outside the range on either side (custom ranges fully in the past or
    // starting today): show the whole range rather than an empty window.
    if midnight <= start || midnight >= end || span <= 0.0 {
        return (0.0, 100.0);
    }
    let pct = (midnight - start).num_milliseconds() as f64 / span * 100.0;
    (pct, 100.0)
}

/// Leveled x-axis label templates: ECharts renders the coarsest applicable
/// level at each tick, so day/month boundaries automatically name the day
/// while plain hours stay short — the day is always visible on the axis
/// without a function formatter.
fn time_axis_label_formatter() -> serde_json::Value {
    serde_json::json!({
        "year": "{yyyy}",
        "month": "{MMM} {d}",
        "day": "{ee} {d}",
        "hour": "{HH}:{mm}",
        "minute": "{HH}:{mm}",
    })
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

    let text = crate::echarts::css_var("--text");
    let muted = crate::echarts::css_var("--muted");
    let border = crate::echarts::css_var("--border");
    let surface = crate::echarts::css_var("--surface");

    // Every room resampled onto one shared time grid. HA sensors report at
    // their own moments, and the axis tooltip only lists series that own a
    // point at the snapped timestamp — raw series made it show a single
    // room. HA history is state-based, so carrying the last value forward
    // is exact, not an approximation; before a room's first reading the
    // value is null (line simply starts later).
    let start_ms = start.timestamp_millis();
    let end_ms = end.timestamp_millis();
    // Dense enough that the default view (7 days loaded, zoomed to today)
    // still resolves ~10-minute detail inside the zoom window.
    let step_ms = ((end_ms - start_ms) / 1_000).max(60_000);

    let series_json: Vec<serde_json::Value> = series
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let mut readings = s.readings.iter().peekable();
            let mut current: Option<f64> = None;
            let mut data = Vec::new();
            let mut t = start_ms;
            while t <= end_ms {
                while readings
                    .peek()
                    .is_some_and(|r| r.at.timestamp_millis() <= t)
                {
                    current = readings.next().map(|r| convert(r.celsius));
                }
                data.push(match current {
                    Some(value) => serde_json::json!([t, value]),
                    None => serde_json::json!([t, serde_json::Value::Null]),
                });
                t += step_ms;
            }
            serde_json::json!({
                "name": s.label,
                "type": "line",
                "showSymbol": false,
                "color": SERIES_COLORS[i % SERIES_COLORS.len()],
                "lineStyle": { "width": 1.5 },
                "data": data,
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
        // Shared gestures — see echarts::inside_zoom.
        "dataZoom": crate::echarts::inside_zoom(),
        "xAxis": {
            "type": "time",
            "min": start.timestamp_millis(),
            "max": end.timestamp_millis(),
            "axisLabel": {
                "color": muted,
                "hideOverlap": true,
                "formatter": time_axis_label_formatter(),
            },
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

/// On/off + brightness + color for one configured light. Holds its own
/// state (seeded from the initial fetch) and applies it optimistically from
/// the server's response after each command — no shared resource
/// invalidation needed across cards.
#[component]
fn LightCard(light: LightState) -> impl IntoView {
    let client = use_client();
    let id = light.id.clone();

    let label = light.label.clone();
    let on = RwSignal::new(light.on);
    let available = RwSignal::new(light.available);
    let brightness = RwSignal::new(light.brightness.unwrap_or(100));
    let color = RwSignal::new(light.color.unwrap_or(RgbColor {
        r: 255,
        g: 255,
        b: 255,
    }));

    let apply_state = move |state: LightState| {
        on.set(state.on);
        available.set(state.available);
        if let Some(b) = state.brightness {
            brightness.set(b);
        }
        if let Some(c) = state.color {
            color.set(c);
        }
    };

    let send = move |cmd: LightCommand| {
        let client = client.clone();
        let id = id.clone();
        spawn_local(async move {
            match client.set_light(&id, &cmd).await {
                Ok(state) => apply_state(state),
                // The optimistic flip must not stand on failure: mark the
                // card unreachable until the next successful command.
                Err(_) => available.set(false),
            }
        });
    };

    // Brightness/color moved while the light is off are only remembered
    // (never sent — adjusting a percentage must not power the light) and
    // ride along with the next turn-on.
    let pending = RwSignal::new(LightCommand::default());

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
    let change_brightness = {
        let send = send.clone();
        move |ev: leptos::ev::Event| {
            if let Ok(pct) = event_target_value(&ev).parse::<u8>() {
                brightness.set(pct);
                if on.get_untracked() {
                    send(LightCommand {
                        brightness: Some(pct),
                        ..Default::default()
                    });
                } else {
                    pending.update(|p| p.brightness = Some(pct));
                }
            }
        }
    };
    let change_color = move |ev: leptos::ev::Event| {
        if let Some(rgb) = parse_hex_color(&event_target_value(&ev)) {
            color.set(rgb);
            if on.get_untracked() {
                send(LightCommand {
                    color: Some(rgb),
                    ..Default::default()
                });
            } else {
                pending.update(|p| p.color = Some(rgb));
            }
        }
    };

    view! {
        <div class="light-card" class:unavailable=move || !available.get()>
            <label class="light-card-head">
                <input type="checkbox" prop:checked=on on:change=toggle/>
                <span class="light-card-label">{label}</span>
            </label>
            <label class="light-card-row">
                <span class="muted">"Brightness"</span>
                <input
                    type="range"
                    min="0"
                    max="100"
                    prop:value=brightness
                    // Live readout while dragging; the command only goes
                    // out on release (change), not per pixel of drag.
                    on:input=move |ev| {
                        if let Ok(pct) = event_target_value(&ev).parse::<u8>() {
                            brightness.set(pct);
                        }
                    }
                    on:change=change_brightness
                />
                <span class="muted light-pct">{move || format!("{}%", brightness.get())}</span>
            </label>
            <label class="light-card-row">
                <span class="muted">"Color"</span>
                <input
                    type="color"
                    prop:value=move || to_hex_color(&color.get())
                    on:change=change_color
                />
            </label>
            {move || (!available.get()).then(|| view! { <p class="error">"Unreachable"</p> })}
        </div>
    }
}

fn parse_hex_color(hex: &str) -> Option<RgbColor> {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    Some(RgbColor {
        r: u8::from_str_radix(&hex[0..2], 16).ok()?,
        g: u8::from_str_radix(&hex[2..4], 16).ok()?,
        b: u8::from_str_radix(&hex[4..6], 16).ok()?,
    })
}

fn to_hex_color(c: &RgbColor) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

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

    #[test]
    fn today_window_shows_a_fully_past_range_whole() {
        // A custom range that ended before today must not zoom to an empty
        // (100, 100) window — double-click couldn't rescue it.
        let start = Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2026, 7, 8, 0, 0, 0).unwrap();
        let midnight = Utc.with_ymd_and_hms(2026, 7, 11, 0, 0, 0).unwrap();
        assert_eq!(today_window_at(midnight, start, end), (0.0, 100.0));
    }

    #[test]
    fn axis_labels_name_the_day_at_boundaries() {
        let fmt = super::time_axis_label_formatter();
        assert_eq!(fmt["day"], "{ee} {d}");
        assert_eq!(fmt["hour"], "{HH}:{mm}");
    }
}
