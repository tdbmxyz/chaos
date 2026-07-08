//! The Home tab: temperature history from Home Assistant sensors (with a
//! date/time range picker) and light control (on/off, brightness, color).
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
    let range = RwSignal::new((now - Duration::hours(24), now));

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
            if let Ok(state) = client.set_light(&id, &cmd).await {
                apply_state(state);
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
                    on:change=change_brightness
                />
                <span class="muted">{move || format!("{}%", brightness.get())}</span>
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
