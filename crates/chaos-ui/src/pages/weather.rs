use chaos_domain::WeatherData;
use chrono::Timelike;
use leptos::prelude::*;

use crate::{WEATHER_LOCATION_KEY, use_client};

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

/// Weather in detail: one row per location with current conditions and the
/// hour-by-hour forecast, so places compare at a glance. The location list
/// is a device preference; with none configured the dashboard's location
/// (device override or server default) is shown alone.
#[component]
pub fn WeatherPage() -> impl IntoView {
    let places = RwSignal::new(crate::weather_places());
    let input = RwSignal::new(String::new());

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
                    view! { <WeatherRow location=None on_remove=None/> }.into_any()
                } else {
                    list.into_iter()
                        .map(|place| {
                            view! { <WeatherRow location=Some(place) on_remove=Some(remove)/> }
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
fn WeatherRow(location: Option<String>, on_remove: Option<Callback<String>>) -> impl IntoView {
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
                Some(Ok(weather)) => weather_row_body(weather).into_any(),
                Some(Err(err)) => {
                    let place = location.clone().unwrap_or_else(|| "default location".into());
                    view! { <p class="error">{place} ": " {err.to_string()}</p> }.into_any()
                }
            }}
            {remove}
        </section>
    }
}

fn weather_row_body(weather: WeatherData) -> impl IntoView {
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
                let option = Callback::new(move |()| weather_chart_option(&hourly, fahrenheit, &colors));
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

    #[test]
    fn option_has_category_axis_and_one_series() {
        let hours = vec![hour(2026, 7, 9, 14, 20.0, 0), hour(2026, 7, 9, 15, 21.0, 2)];
        // Empty colours: the test asserts structure, not styling, and keeps the
        // DOM-reading css_var out of this native test.
        let opt = weather_chart_option(&hours, false, &crate::echarts::ChartColors::default());
        assert_eq!(opt["xAxis"]["type"], "category");
        assert_eq!(opt["xAxis"]["axisLabel"]["interval"], 2);
        assert_eq!(opt["series"].as_array().unwrap().len(), 1);
        assert_eq!(opt["series"][0]["data"], serde_json::json!([20.0, 21.0]));
        assert_eq!(
            opt["xAxis"]["data"],
            serde_json::json!([
                format!("{}\n14h", crate::weather_emoji(0)),
                format!("{}\n15h", crate::weather_emoji(2)),
            ])
        );
    }
}
