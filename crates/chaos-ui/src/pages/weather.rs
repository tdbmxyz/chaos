use chaos_domain::WeatherData;
use chrono::Timelike;
use leptos::prelude::*;

use crate::{WEATHER_LOCATION_KEY, use_client};

/// Every loaded location's hourly forecast, insertion-ordered as fetches
/// resolve; keyed by the API's resolved display name. Charts read it for
/// the combined tooltip and the shared y-range.
type LoadedForecasts = Vec<(String, Vec<chaos_domain::HourlyForecast>)>;

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
    let temps = everyone
        .iter()
        .flat_map(|(_, h)| hourly_temps(h, fahrenheit));
    let (min, max) = temps.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), t| {
        (lo.min(t), hi.max(t))
    });
    debug_assert!(min.is_finite(), "y_range needs at least one data point");
    ((min - 1.0).floor(), (max + 1.0).ceil())
}

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
    all: &LoadedForecasts,
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
                // Always draw the own line from the row's fresh data: a
                // refetch could leave the shared entry transiently stale,
                // and the x-axis labels already come from `own_hourly`.
                own_series(own_hourly)
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

/// Weather in detail: one row per location with current conditions and the
/// hour-by-hour forecast, so places compare at a glance. The location list
/// is a device preference; with none configured the dashboard's location
/// (device override or server default) is shown alone.
#[component]
pub fn WeatherPage() -> impl IntoView {
    let places = RwSignal::new(crate::weather_places());
    let input = RwSignal::new(String::new());
    let loaded = RwSignal::new(LoadedForecasts::new());

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
            </div>
            {move || {
                let list = places.get();
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
            }}
        </div>
    }
}

/// One location: current conditions plus the hourly strip (48 h, scrolls
/// sideways; midnight cells carry the weekday instead of an hour).
#[component]
fn WeatherRow(
    location: Option<String>,
    on_remove: Option<Callback<String>>,
    loaded: RwSignal<LoadedForecasts>,
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
                Some(Ok(weather)) => weather_row_body(weather, loaded).into_any(),
                Some(Err(err)) => {
                    let place = location.clone().unwrap_or_else(|| "default location".into());
                    view! { <p class="error">{place} ": " {err.to_string()}</p> }.into_any()
                }
            }}
            {remove}
        </section>
    }
}

fn weather_row_body(weather: WeatherData, loaded: RwSignal<LoadedForecasts>) -> impl IntoView {
    let location = weather.location.clone();
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
            if hourly.is_empty() {
                // Mirror the Home chart's empty state rather than showing an
                // axes-only, line-less box.
                view! { <p class="muted">"No hourly forecast."</p> }.into_any()
            } else {
                let colors = crate::echarts::ChartColors::from_theme();
                let option = Callback::new(move |()| {
                    let all = loaded.get();
                    weather_chart_option(&location, &hourly, &all, fahrenheit, &colors)
                });
                view! {
                    <crate::echarts::ChartCanvas option group="weather" class="weather-chart"/>
                }
                .into_any()
            }
        }
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
    fn labels_show_weekday_at_midnight() {
        // 2026-07-10 is a Friday; midnight rows carry the weekday, not "0h".
        let hours = vec![hour(2026, 7, 10, 0, 15.0, 2)];
        assert_eq!(
            hourly_labels(&hours),
            vec![format!("{}\nFri", crate::weather_emoji(2))]
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
        let all = [("A".to_string(), vec![hour(2026, 7, 9, 14, 20.0, 0)])];
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
        // "Nijar" is index 1 in the shared list on both charts; in opt_b the
        // own series is prepended, shifting it to position 2 — same color.
        assert_eq!(opt_a["series"][1]["name"], "Nijar");
        assert_eq!(opt_b["series"][2]["name"], "Nijar");
        assert_eq!(opt_a["series"][1]["color"], opt_b["series"][2]["color"]);
    }
}
