use chaos_domain::WeatherData;
use chrono::Timelike;
use leptos::prelude::*;

use crate::{WEATHER_LOCATION_KEY, use_client};

/// Every loaded location's hourly forecast plus its `now_index`, insertion-
/// ordered as fetches resolve; keyed by the API's resolved display name.
/// Charts read it for the shared y-range and the combined view.
type LoadedForecasts = Vec<(String, Vec<chaos_domain::HourlyForecast>, usize)>;

/// Two-line x-axis labels: weather emoji on top, then the hour (`"14h"`), or
/// the weekday and day-of-month ("Fri 10") at midnight so day boundaries read
/// at a glance.
fn hourly_labels(hourly: &[chaos_domain::HourlyForecast]) -> Vec<String> {
    hourly
        .iter()
        .map(|h| {
            let below = if h.time.hour() == 0 {
                // "Fri 10" — day boundaries stay identifiable across weeks.
                h.time.format("%a %-d").to_string()
            } else {
                format!("{}h", h.time.hour())
            };
            format!("{}\n{}", crate::weather_emoji(h.weather_code), below)
        })
        .collect()
}

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

/// Line colors for the combined chart, one per location by list index.
const LOCATION_PALETTE: [&str; 6] = [
    "#5470c6", "#91cc75", "#fac858", "#ee6666", "#73c0de", "#9a60b4",
];

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

/// The page-wide y-axis bounds: min/max over every location's converted
/// temperatures, padded one display degree and rounded outward. Every chart
/// pins its y-axis to this, so scales match and zooming never rescales.
/// Precondition: at least one non-empty series (callers guard empty hourly).
fn y_range(everyone: &[(&str, &[chaos_domain::HourlyForecast])], fahrenheit: bool) -> (f64, f64) {
    let temps = everyone
        .iter()
        .flat_map(|(_, h)| hourly_temps(h, fahrenheit));
    let (min, max) = temps.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), t| {
        (lo.min(t), hi.max(t))
    });
    debug_assert!(min.is_finite(), "y_range needs at least one data point");
    ((min - 1.0).floor(), (max + 1.0).ceil())
}

/// The default visible window — past 24 h through next 48 h — as dataZoom
/// percentages of the full series, clamped to [0, 100]. Full range for an
/// empty series.
fn default_window(now_index: usize, len: usize) -> (f64, f64) {
    if len == 0 {
        return (0.0, 100.0);
    }
    let len = len as f64;
    // Multiply before dividing: `58.0 / 100.0 * 100.0` is 57.999…, while
    // `58.0 * 100.0 / 100.0` stays exact.
    let start = (now_index as f64 - 24.0).max(0.0) * 100.0 / len;
    let end = (now_index as f64 + 48.0).min(len) * 100.0 / len;
    (start, end)
}

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
    everyone.push(("", hourly)); // y-range only; names unused there, duplicates harmless.
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
            // now_index may equal len (all past); ECharts clips the out-of-range line.
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
                // `now_index` may equal len (all data in the past); ECharts
                // clips the out-of-range mark line instead of erroring.
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

/// Weather in detail: one row per location with current conditions and the
/// hour-by-hour forecast, so places compare at a glance. The location list
/// is a device preference; with none configured the dashboard's location
/// (device override or server default) is shown alone.
#[component]
pub fn WeatherPage() -> impl IntoView {
    let places = RwSignal::new(crate::weather_places());
    let input = RwSignal::new(String::new());
    let loaded = RwSignal::new(LoadedForecasts::new());
    let combined = RwSignal::new(false);

    let add = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let name = input.get_untracked().trim().to_string();
        if name.is_empty() {
            return;
        }
        places.update(|list| {
            if !list.contains(&name) {
                list.push(name);
            }
        });
        crate::set_weather_places(&places.get_untracked());
        input.set(String::new());
    };
    let remove = Callback::new(move |name: String| {
        places.update(|list| list.retain(|p| p != &name));
        crate::set_weather_places(&places.get_untracked());
    });

    view! {
        <div class="weather-page">
            <div class="weather-page-head">
                <h2>"Weather"</h2>
                <form class="weather-add" on:submit=add>
                    <input
                        type="text"
                        placeholder="Add a location — e.g. Osaka, JP"
                        prop:value=input
                        on:input=move |ev| input.set(event_target_value(&ev))
                    />
                    <button type="submit">"Add"</button>
                </form>
                <button
                    class="view-toggle"
                    title="Switch between one chart per place and one combined chart"
                    on:click=move |_| combined.update(|c| *c = !*c)
                >
                    {move || if combined.get() { "Split" } else { "Combine" }}
                </button>
            </div>
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
        </div>
    }
}

/// One location: current conditions plus the ±16-day hourly chart.
#[component]
fn WeatherRow(
    location: Option<String>,
    on_remove: Option<Callback<String>>,
    loaded: RwSignal<LoadedForecasts>,
    combined: RwSignal<bool>,
) -> impl IntoView {
    let client = use_client();
    // A configured row asks for its place; the default row follows the
    // device preference (or the server's location when unset).
    let query = location
        .clone()
        .or_else(|| crate::pref(WEATHER_LOCATION_KEY));
    let data = LocalResource::new(move || {
        let client = client.clone();
        let query = query.clone();
        async move { client.weather(query.as_deref()).await }
    });

    // Publish this row's forecast into the page-wide list (charts read it
    // for the shared y-range and the combined view). Upsert by resolved name
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
    });
    on_cleanup(move || {
        if let Some(name) = published.get_value() {
            loaded.update(|list| list.retain(|(n, _, _)| n != &name));
        }
    });

    let remove = location.clone().zip(on_remove).map(|(name, on_remove)| {
        view! {
            <button
                class="icon-btn"
                title="Remove this location"
                on:click=move |_| on_remove.run(name.clone())
            >
                "✕"
            </button>
        }
    });

    view! {
        <section class="weather-row">
            {move || match data.get() {
                None => view! { <p class="muted">"Loading forecast…"</p> }.into_any(),
                Some(Ok(weather)) => weather_row_body(weather, loaded, combined).into_any(),
                Some(Err(err)) => {
                    let place = location.clone().unwrap_or_else(|| "default location".into());
                    view! { <p class="error">{place} ": " {err.to_string()}</p> }.into_any()
                }
            }}
            {remove}
        </section>
    }
}

fn weather_row_body(
    weather: WeatherData,
    loaded: RwSignal<LoadedForecasts>,
    combined: RwSignal<bool>,
) -> impl IntoView {
    let fahrenheit = crate::weather_fahrenheit();
    let temp = move |celsius: f64| crate::format_temp(celsius, fahrenheit);
    let details = format!(
        "{} · feels {} · wind {:.0} km/h{}",
        weather.description,
        temp(weather.apparent_c),
        weather.wind_kmh,
        weather
            .humidity_pct
            .map(|h| format!(" · {h:.0}% humidity"))
            .unwrap_or_default(),
    );

    view! {
        <div class="weather-row-head">
            <span class="weather-emoji">{crate::weather_emoji(weather.weather_code)}</span>
            <div class="weather-row-title">
                <h3>{weather.location}</h3>
                <span class="muted">{details}</span>
            </div>
            <span class="weather-row-temp">{temp(weather.temperature_c)}</span>
        </div>
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
            }
        }
    }
}

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
        // 2026-07-09 14:00 -> emoji for code 0 over "14h".
        let hours = vec![hour(2026, 7, 9, 14, 20.0, 0)];
        assert_eq!(
            hourly_labels(&hours),
            vec![format!("{}\n14h", crate::weather_emoji(0))]
        );
    }

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
        // now == len (all data in the past) still yields a valid window.
        assert_eq!(default_window(100, 100), (76.0, 100.0));
    }

    #[test]
    fn default_window_full_range_when_empty() {
        assert_eq!(default_window(0, 0), (0.0, 100.0));
    }

    #[test]
    fn split_option_has_one_series_and_now_marker() {
        let all = two_locations();
        let own = all[0].1.clone();
        let opt = weather_chart_option(
            &own,
            1,
            &all,
            false,
            &crate::echarts::ChartColors::default(),
        );
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
        // x-axis labels come from the own row's data.
        assert_eq!(
            opt["xAxis"]["data"][0],
            format!("{}\n14h", crate::weather_emoji(0))
        );
    }

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
        let hours = vec![
            hour(2026, 7, 10, 0, 15.0, 2),
            hour(2026, 7, 10, 14, 20.0, 0),
        ];
        assert_eq!(time_labels(&hours), vec!["Fri 10", "14h"]);
    }

    #[test]
    fn split_option_y_range_includes_own_unpublished_data() {
        // Own data not in the shared list still shapes the y-range.
        let all = two_locations();
        let own = vec![hour(2026, 7, 9, 14, 10.0, 0)];
        let opt = weather_chart_option(
            &own,
            0,
            &all,
            false,
            &crate::echarts::ChartColors::default(),
        );
        assert_eq!(opt["yAxis"]["min"], 9.0);
        assert_eq!(opt["yAxis"]["max"], 32.0);
    }
}
