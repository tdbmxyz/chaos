use chaos_domain::WeatherData;
use leptos::prelude::*;

const HOUR_MS: i64 = 3_600_000;

/// One loaded location published into the page-wide list: its hourly
/// forecast, `now_index` (whose only reader is now `CombinedChart`'s
/// remount key), and the place's UTC offset (Open-Meteo hourly times are
/// location-local; the charts convert them back to real instants); keyed
/// by the API's resolved display name.
#[derive(Clone)]
struct LoadedPlace {
    name: String,
    hourly: Vec<chaos_domain::HourlyForecast>,
    now_index: usize,
    utc_offset_seconds: i32,
}

/// Every loaded location, insertion-ordered as fetches resolve. Charts read
/// it for the shared y-range, the shared time span, and the combined view.
type LoadedPlaces = Vec<LoadedPlace>;

/// Milliseconds since epoch of a location-local forecast hour: Open-Meteo
/// hands back local wall-clock times, so subtracting the place's UTC offset
/// recovers the real instant every chart plots on.
fn utc_ms(local: chrono::NaiveDateTime, utc_offset_seconds: i32) -> i64 {
    (local - chrono::Duration::seconds(utc_offset_seconds as i64))
        .and_utc()
        .timestamp_millis()
}

/// `[ts, temp, emoji]` triples for a time-axis series: the timestamp is the
/// real UTC instant, the temperature is in the display unit at one decimal,
/// and the emoji rides along for the tooltip (`chaosWeatherTooltip`).
fn series_points(
    hourly: &[chaos_domain::HourlyForecast],
    utc_offset_seconds: i32,
    fahrenheit: bool,
) -> Vec<serde_json::Value> {
    hourly
        .iter()
        .map(|h| {
            serde_json::json!([
                utc_ms(h.time, utc_offset_seconds),
                crate::convert_temp_1dp(h.temp_c, fahrenheit),
                crate::weather_emoji(h.weather_code),
            ])
        })
        .collect()
}

/// The union `[min, max]` in epoch ms over every loaded place (plus the own
/// row's not-yet-published data, mirroring `y_range`). Every chart pins its
/// x-axis to this span, so the percent-based zoom sync of the `weather`
/// connect group keeps the same real instant under the cursor everywhere.
/// `None` when there is no data at all (callers fall back to a full-range
/// window).
fn axis_span(
    all: &LoadedPlaces,
    own: Option<(&[chaos_domain::HourlyForecast], i32)>,
) -> Option<(i64, i64)> {
    let everyone = all
        .iter()
        .map(|p| (p.hourly.as_slice(), p.utc_offset_seconds))
        .chain(own);
    everyone
        .flat_map(|(hourly, offset)| hourly.iter().map(move |h| utc_ms(h.time, offset)))
        .fold(None, |span, ms| {
            Some(match span {
                Some((lo, hi)) => (lo.min(ms), hi.max(ms)),
                None => (ms, ms),
            })
        })
}

/// The default visible window — past 24 h through next 48 h around `now_ms`
/// — as dataZoom percentages of the pinned `[min_ms, max_ms]` axis span,
/// clamped to [0, 100]. Full range for a degenerate span.
fn default_window_ms(now_ms: i64, min_ms: i64, max_ms: i64) -> (f64, f64) {
    let span = (max_ms - min_ms) as f64;
    if span <= 0.0 {
        return (0.0, 100.0);
    }
    let pct = |ms: i64| (((ms - min_ms) as f64) * 100.0 / span).clamp(0.0, 100.0);
    (pct(now_ms - 24 * HOUR_MS), pct(now_ms + 48 * HOUR_MS))
}

/// Alternating viewer-local day bands over `[min_ms, max_ms]`: markArea
/// pairs `[[{xAxis: start}, {xAxis: end}], …]` shading every OTHER calendar
/// day so day boundaries read at a glance on the time axis. The first band
/// opens at the viewer-local midnight at-or-before `min_ms` (ECharts clips
/// it to the axis); the last is clipped to `max_ms`. `viewer_offset_seconds`
/// comes from `viewer_clock` at call sites — a parameter so this stays pure.
/// A DST shift inside the 32-day span moves band edges by an hour —
/// acceptable.
fn day_bands(min_ms: i64, max_ms: i64, viewer_offset_seconds: i32) -> serde_json::Value {
    const DAY_MS: i64 = 86_400_000;
    let offset_ms = viewer_offset_seconds as i64 * 1000;
    // Viewer-local midnight at or before min_ms (div_euclid: correct for
    // instants before the epoch too).
    let first_midnight = (min_ms + offset_ms).div_euclid(DAY_MS) * DAY_MS - offset_ms;
    let bands = (0..)
        .map(|i| first_midnight + 2 * i * DAY_MS)
        .take_while(|start| *start < max_ms)
        .map(|start| {
            serde_json::json!([
                { "xAxis": start },
                { "xAxis": (start + DAY_MS).min(max_ms) },
            ])
        })
        .collect();
    serde_json::Value::Array(bands)
}

/// The dashed "now" markLine both option builders hang on a series: a
/// vertical line at the real instant separating past from forecast.
fn now_mark_line(now_ms: i64, muted: &str) -> serde_json::Value {
    serde_json::json!({
        "silent": true,
        "symbol": "none",
        "label": { "show": true, "formatter": "now", "color": muted },
        "lineStyle": { "color": muted, "type": "dashed", "width": 1 },
        "data": [{ "xAxis": now_ms }],
    })
}

/// The alternating day-band markArea both option builders hang on a series
/// (bands from `day_bands`, faintly filled with the border color).
fn day_band_mark_area(
    min_ms: i64,
    max_ms: i64,
    viewer_offset_seconds: i32,
    border: &str,
) -> serde_json::Value {
    serde_json::json!({
        "silent": true,
        "itemStyle": { "color": border, "opacity": 0.08 },
        "data": day_bands(min_ms, max_ms, viewer_offset_seconds),
    })
}

/// The viewer's clock and time zone, read from the browser: `(now_ms,
/// viewer_offset_seconds)`. The offset is `-(js Date.getTimezoneOffset() *
/// 60)` — JS reports minutes *behind* UTC, so the sign flips to the usual
/// east-positive convention. Browser-only; components call it so the option
/// builders stay pure/testable.
fn viewer_clock() -> (i64, i32) {
    let now_ms = js_sys::Date::now() as i64;
    let viewer_offset_seconds = -(js_sys::Date::new_0().get_timezone_offset() as i32) * 60;
    (now_ms, viewer_offset_seconds)
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
        .map(|h| crate::convert_temp_1dp(h.temp_c, fahrenheit))
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

/// The ECharts option for one location in split view. Time x-axis with
/// `[ts, temp, emoji]` points on real UTC instants, pinned to the union
/// span over every loaded place — same x means same real instant on every
/// chart, and the connect group's percent-based zoom sync stays exact
/// because every chart shares min/max. One visible series named after the
/// location (the tooltip shows the name); the y-axis is pinned to the
/// page-wide range (every loaded location, plus this row's own data in case
/// its publish hasn't landed — duplicates can't move a min/max) so charts
/// compare at a glance. A dashed "now" mark line separates past from
/// forecast; alternating viewer-local day bands mark calendar days. `now_ms`
/// and `viewer_offset_seconds` are parameters (with the colours) so this
/// stays pure/testable — components read the clock and time zone.
fn weather_chart_option(
    own: &LoadedPlace,
    all: &LoadedPlaces,
    now_ms: i64,
    viewer_offset_seconds: i32,
    fahrenheit: bool,
    colors: &crate::echarts::ChartColors,
) -> serde_json::Value {
    let unit = if fahrenheit { "°F" } else { "°C" };

    let text = colors.text.as_str();
    let muted = colors.muted.as_str();
    let border = colors.border.as_str();
    let surface = colors.surface.as_str();
    let accent = colors.accent.as_str();

    let mut everyone: Vec<(&str, &[chaos_domain::HourlyForecast])> = all
        .iter()
        .map(|p| (p.name.as_str(), p.hourly.as_slice()))
        .collect();
    everyone.push(("", &own.hourly)); // y-range only; names unused there, duplicates harmless.
    let (y_min, y_max) = y_range(&everyone, fahrenheit);
    // Callers guard non-empty own data, so the span always exists.
    let Some((min_ms, max_ms)) = axis_span(all, Some((&own.hourly, own.utc_offset_seconds))) else {
        return serde_json::json!({});
    };

    serde_json::json!({
        "animation": false,
        "grid": { "left": 44, "right": 16, "top": 20, "bottom": 40 },
        // The formatter is grafted on by ChartCanvas (chaosWeatherTooltip) —
        // the JSON bridge can't carry a JS function.
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
            "min": min_ms,
            "max": max_ms,
            // Auto-thinned labels: density adapts to the zoom level across
            // the 32-day series.
            "axisLabel": { "color": muted, "hideOverlap": true },
            "axisLine": { "lineStyle": { "color": border } },
        },
        "yAxis": {
            "type": "value",
            "min": y_min,
            "max": y_max,
            "axisLabel": { "color": muted, "formatter": format!("{{value}}{unit}") },
            "splitLine": { "lineStyle": { "color": border } },
        },
        "series": [{
            "name": own.name,
            "type": "line",
            "showSymbol": false,
            "color": accent,
            "lineStyle": { "width": 1.5 },
            "data": series_points(&own.hourly, own.utc_offset_seconds, fahrenheit),
            "markLine": now_mark_line(now_ms, muted),
            "markArea": day_band_mark_area(min_ms, max_ms, viewer_offset_seconds, border),
        }],
    })
}

/// The combined view: every loaded location as a visible line in one chart,
/// with a legend and the multi-series axis tooltip doing the comparison.
/// Same pinned y-range, pinned time span, zoom gestures, now-marker, and
/// day bands (the latter two on the first series) as the split charts. Each
/// place's points use its OWN UTC offset, so cross-timezone locations align
/// by real instant, not by hour index. An empty `all` (possible transiently
/// while the last row retracts) yields an empty option rather than
/// panicking; the caller normally renders an empty state instead.
fn combined_chart_option(
    all: &LoadedPlaces,
    now_ms: i64,
    viewer_offset_seconds: i32,
    fahrenheit: bool,
    colors: &crate::echarts::ChartColors,
) -> serde_json::Value {
    let unit = if fahrenheit { "°F" } else { "°C" };
    // Retraction can empty the list before the chart unmounts (framework
    // scheduling); render an empty option instead of indexing into nothing.
    let Some((min_ms, max_ms)) = axis_span(all, None) else {
        return serde_json::json!({});
    };

    let text = colors.text.as_str();
    let muted = colors.muted.as_str();
    let border = colors.border.as_str();
    let surface = colors.surface.as_str();

    let everyone: Vec<(&str, &[chaos_domain::HourlyForecast])> = all
        .iter()
        .map(|p| (p.name.as_str(), p.hourly.as_slice()))
        .collect();
    let (y_min, y_max) = y_range(&everyone, fahrenheit);

    let names: Vec<&str> = all.iter().map(|p| p.name.as_str()).collect();
    let series: Vec<serde_json::Value> = all
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mut s = serde_json::json!({
                "name": p.name,
                "type": "line",
                "showSymbol": false,
                "color": LOCATION_PALETTE[i % LOCATION_PALETTE.len()],
                "lineStyle": { "width": 1.5 },
                "data": series_points(&p.hourly, p.utc_offset_seconds, fahrenheit),
            });
            if i == 0 {
                // The chart-wide decorations ride the first series once.
                s["markLine"] = now_mark_line(now_ms, muted);
                s["markArea"] = day_band_mark_area(min_ms, max_ms, viewer_offset_seconds, border);
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
        // Shared gestures — see echarts::inside_zoom.
        "dataZoom": crate::echarts::inside_zoom(),
        "xAxis": {
            "type": "time",
            "min": min_ms,
            "max": max_ms,
            "axisLabel": { "color": muted, "hideOverlap": true },
            "axisLine": { "lineStyle": { "color": border } },
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
    let loaded = RwSignal::new(LoadedPlaces::new());
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
                    view! {
                        <For
                            each=move || places.get()
                            key=|place| place.clone()
                            children=move |place| {
                                view! {
                                    <WeatherRow
                                        location=Some(place)
                                        on_remove=Some(remove)
                                        loaded
                                        combined
                                    />
                                }
                            }
                        />
                    }
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
    loaded: RwSignal<LoadedPlaces>,
    combined: RwSignal<bool>,
) -> impl IntoView {
    // A configured row asks for its place; the default row follows the
    // device preference (or the dashboard's configured location when unset).
    let query = location.clone();
    let client = crate::use_client();
    let data = LocalResource::new(move || {
        let query = query.clone();
        let client = client.clone();
        async move {
            let place = match query {
                Some(place) => place,
                None => match crate::weather_fetch::default_location(&client).await {
                    Some(place) => place,
                    None => return Err("no location set — add one in settings".to_string()),
                },
            };
            crate::weather_fetch::place_weather(&place).await
        }
    });

    // Publish this row's forecast into the page-wide list (charts read it
    // for the shared y-range and the combined view). Upsert by resolved name
    // so refetches don't duplicate; remember the name to unpublish when the
    // row unmounts (location removed / page left).
    let published = StoredValue::new(None::<String>);
    Effect::new(move |_| {
        let Some(Ok((weather, utc_offset_seconds))) = data.get() else {
            return;
        };
        if weather.hourly.is_empty() {
            return;
        }
        let place = LoadedPlace {
            name: weather.location,
            hourly: weather.hourly,
            now_index: weather.now_index,
            utc_offset_seconds,
        };
        published.set_value(Some(place.name.clone()));
        // Known edge: two place strings resolving to the same display name
        // share one entry, and removing one row retracts the entry the other
        // still uses — self-healing, because the y-range folds each row's
        // own data in regardless.
        loaded.update(
            |list| match list.iter_mut().find(|p| p.name == place.name) {
                Some(entry) => *entry = place,
                None => list.push(place),
            },
        );
    });
    on_cleanup(move || {
        if let Some(name) = published.get_value() {
            loaded.update(|list| list.retain(|p| p.name != name));
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
                Some(Ok((weather, utc_offset_seconds))) => {
                    weather_row_body(weather, utc_offset_seconds, loaded, combined).into_any()
                }
                Some(Err(err)) => {
                    let place = location.clone().unwrap_or_else(|| "default location".into());
                    view! { <p class="error">{place} ": " {err}</p> }.into_any()
                }
            }}
            {remove}
        </section>
    }
}

fn weather_row_body(
    weather: WeatherData,
    utc_offset_seconds: i32,
    loaded: RwSignal<LoadedPlaces>,
    combined: RwSignal<bool>,
) -> impl IntoView {
    let fahrenheit = crate::weather_fahrenheit();
    let temp = move |celsius: f64| crate::format_temp(celsius, fahrenheit);
    let details = crate::weather_details(&weather.description, &weather, fahrenheit);

    view! {
        <div class="weather-row-head">
            <span class="weather-emoji">{crate::weather_emoji(weather.weather_code)}</span>
            <div class="weather-row-title">
                <h3>{weather.location.clone()}</h3>
                <span class="muted">{details}</span>
            </div>
            <span class="weather-row-temp">{temp(weather.temperature_c)}</span>
        </div>
        {
            if weather.hourly.is_empty() {
                // Mirror the Home chart's empty state rather than showing an
                // axes-only, line-less box.
                view! { <p class="muted">"No hourly forecast."</p> }.into_any()
            } else {
                // Same shape as the published entry — the builder folds it
                // into the ranges in case its publish hasn't landed yet.
                let own = LoadedPlace {
                    name: weather.location,
                    hourly: weather.hourly,
                    now_index: weather.now_index,
                    utc_offset_seconds,
                };
                let colors = crate::echarts::ChartColors::from_theme();
                let (now_ms, viewer_offset_seconds) = viewer_clock();
                let reset = axis_span(
                    &loaded.get_untracked(),
                    Some((&own.hourly, own.utc_offset_seconds)),
                )
                .map(|(min_ms, max_ms)| default_window_ms(now_ms, min_ms, max_ms))
                .unwrap_or((0.0, 100.0));
                let option = Callback::new(move |()| {
                    loaded.with(|all| {
                        weather_chart_option(
                            &own,
                            all,
                            now_ms,
                            viewer_offset_seconds,
                            fahrenheit,
                            &colors,
                        )
                    })
                });
                view! {
                    <Show when=move || !combined.get()>
                        <crate::echarts::ChartCanvas
                            option
                            group="weather"
                            reset_zoom=reset
                            tooltip_formatter="chaosWeatherTooltip"
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
/// callback reads `loaded` so those updates are reactive. A `now_index`
/// change (a refetch crossing the hour) remounting the chart is useful:
/// it refreshes the mount-captured `now_ms` and reset window.
#[component]
fn CombinedChart(loaded: RwSignal<LoadedPlaces>) -> impl IntoView {
    let fahrenheit = crate::weather_fahrenheit();
    let first =
        Memo::new(move |_| loaded.with(|list| list.first().map(|p| (p.hourly.len(), p.now_index))));
    view! {
        <section class="weather-row">
            {move || match first.get() {
                None => view! { <p class="muted">"Loading forecast…"</p> }.into_any(),
                Some(_) => {
                    let colors = crate::echarts::ChartColors::from_theme();
                    let (now_ms, viewer_offset_seconds) = viewer_clock();
                    let reset = axis_span(&loaded.get_untracked(), None)
                        .map(|(min_ms, max_ms)| default_window_ms(now_ms, min_ms, max_ms))
                        .unwrap_or((0.0, 100.0));
                    let option = Callback::new(move |()| {
                        loaded.with(|all| {
                            combined_chart_option(
                                all,
                                now_ms,
                                viewer_offset_seconds,
                                fahrenheit,
                                &colors,
                            )
                        })
                    });
                    view! {
                        <crate::echarts::ChartCanvas
                            option
                            reset_zoom=reset
                            tooltip_formatter="chaosWeatherTooltip"
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

    const HOUR_MS: i64 = 3_600_000;

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

    /// UTC epoch milliseconds of a UTC wall-clock time — the expected values
    /// the time-axis helpers must produce.
    fn ms(y: i32, m: u32, d: u32, h: u32) -> i64 {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis()
    }

    #[test]
    fn utc_ms_subtracts_the_location_offset() {
        // Local 14:00 at UTC+2 is 12:00 UTC.
        let local = hour(2026, 7, 9, 14, 20.0, 0).time;
        assert_eq!(utc_ms(local, 7200), ms(2026, 7, 9, 12));
        // Zero offset is the identity.
        assert_eq!(utc_ms(local, 0), ms(2026, 7, 9, 14));
        // Negative offsets (west of Greenwich) shift the other way.
        assert_eq!(utc_ms(local, -3600), ms(2026, 7, 9, 15));
    }

    #[test]
    fn series_points_carry_ts_temp_and_emoji() {
        let hours = vec![hour(2026, 7, 9, 14, 20.04, 0)];
        assert_eq!(
            series_points(&hours, 7200, false),
            vec![serde_json::json!([
                ms(2026, 7, 9, 12),
                20.0,
                crate::weather_emoji(0)
            ])]
        );
    }

    #[test]
    fn series_points_convert_to_fahrenheit() {
        // 20°C -> 68°F.
        let hours = vec![hour(2026, 7, 9, 14, 20.0, 0)];
        assert_eq!(series_points(&hours, 0, true)[0][1], 68.0);
    }

    #[test]
    fn axis_span_unions_across_offsets() {
        // Berlin (UTC+2): local 14–15 h → 12:00–13:00 UTC; Nijar (UTC):
        // 14:00–15:00 UTC. Union = [12:00, 15:00] — real instants, not the
        // identical-looking local hours.
        let all = two_locations();
        assert_eq!(
            axis_span(&all, None),
            Some((ms(2026, 7, 9, 12), ms(2026, 7, 9, 15)))
        );
    }

    #[test]
    fn axis_span_folds_in_the_own_rows_data() {
        // A not-yet-published row extending past everyone widens the span.
        let all = two_locations();
        let own = vec![hour(2026, 7, 9, 20, 25.0, 0)];
        assert_eq!(
            axis_span(&all, Some((&own, 0))),
            Some((ms(2026, 7, 9, 12), ms(2026, 7, 9, 20)))
        );
    }

    #[test]
    fn axis_span_is_none_when_nothing_is_loaded() {
        assert_eq!(axis_span(&LoadedPlaces::new(), None), None);
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

    /// Two loaded locations for option tests: "Berlin" (20–21°C, UTC+2) and
    /// "Nijar" (30–31°C, UTC), each with now at index 1. Distinct offsets, so
    /// the time-axis assertions can tell the places apart by real instant.
    fn two_locations() -> LoadedPlaces {
        vec![
            LoadedPlace {
                name: "Berlin".to_string(),
                hourly: vec![hour(2026, 7, 9, 14, 20.0, 0), hour(2026, 7, 9, 15, 21.0, 2)],
                now_index: 1,
                utc_offset_seconds: 7200,
            },
            LoadedPlace {
                name: "Nijar".to_string(),
                hourly: vec![hour(2026, 7, 9, 14, 30.0, 1), hour(2026, 7, 9, 15, 31.0, 1)],
                now_index: 1,
                utc_offset_seconds: 0,
            },
        ]
    }

    #[test]
    fn y_range_spans_all_locations_padded_outward() {
        let all = two_locations();
        let everyone: Vec<(&str, &[HourlyForecast])> = all
            .iter()
            .map(|p| (p.name.as_str(), p.hourly.as_slice()))
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
        // Span 0–100 h, now at 50 h → [26 h, 98 h] → percent (26, 98).
        assert_eq!(
            default_window_ms(50 * HOUR_MS, 0, 100 * HOUR_MS),
            (26.0, 98.0)
        );
    }

    #[test]
    fn default_window_clamps_at_span_start() {
        // now at 10 h: past-24h reaches before the span → clamp to 0.
        assert_eq!(
            default_window_ms(10 * HOUR_MS, 0, 100 * HOUR_MS),
            (0.0, 58.0)
        );
    }

    #[test]
    fn default_window_clamps_at_span_end() {
        // now at 90 h: next-48h reaches past the span → clamp to 100.
        assert_eq!(
            default_window_ms(90 * HOUR_MS, 0, 100 * HOUR_MS),
            (66.0, 100.0)
        );
        // now past the span end (all data in the past) still yields a window.
        assert_eq!(
            default_window_ms(130 * HOUR_MS, 0, 100 * HOUR_MS),
            (100.0, 100.0)
        );
    }

    #[test]
    fn default_window_full_range_when_span_is_degenerate() {
        assert_eq!(default_window_ms(0, 0, 0), (0.0, 100.0));
        assert_eq!(default_window_ms(0, 100, 0), (0.0, 100.0));
    }

    #[test]
    fn day_bands_start_at_viewer_midnight_and_alternate() {
        // Span 07-09 06:00 → 07-12 18:00 (UTC viewer): the first band opens
        // at the midnight at-or-before the span start; every other calendar
        // day is shaded (09th, 11th — the 10th and 12th are gaps).
        let bands = day_bands(ms(2026, 7, 9, 6), ms(2026, 7, 12, 18), 0);
        assert_eq!(
            bands,
            serde_json::json!([
                [{ "xAxis": ms(2026, 7, 9, 0) }, { "xAxis": ms(2026, 7, 10, 0) }],
                [{ "xAxis": ms(2026, 7, 11, 0) }, { "xAxis": ms(2026, 7, 12, 0) }],
            ])
        );
    }

    #[test]
    fn day_bands_clip_the_last_band_to_the_span_end() {
        // Max falls mid-day inside a shaded band → the band ends at max.
        let bands = day_bands(ms(2026, 7, 9, 6), ms(2026, 7, 11, 18), 0);
        assert_eq!(
            bands,
            serde_json::json!([
                [{ "xAxis": ms(2026, 7, 9, 0) }, { "xAxis": ms(2026, 7, 10, 0) }],
                [{ "xAxis": ms(2026, 7, 11, 0) }, { "xAxis": ms(2026, 7, 11, 18) }],
            ])
        );
    }

    #[test]
    fn day_bands_use_the_viewer_local_midnight() {
        // Viewer at UTC+2: 07-09 10:00 UTC is 12:00 local, so the local
        // midnight at-or-before it is 07-08 22:00 UTC.
        let bands = day_bands(ms(2026, 7, 9, 10), ms(2026, 7, 9, 20), 7200);
        assert_eq!(bands[0][0]["xAxis"], ms(2026, 7, 8, 22));
        // One local day covers the whole span → a single band, clipped.
        assert_eq!(bands[0][1]["xAxis"], ms(2026, 7, 9, 20));
        assert_eq!(bands.as_array().unwrap().len(), 1);
    }

    #[test]
    fn split_option_uses_a_time_axis_named_after_the_place() {
        let all = two_locations();
        let now_ms = ms(2026, 7, 9, 13);
        let opt = weather_chart_option(
            &all[0],
            &all,
            now_ms,
            0,
            false,
            &crate::echarts::ChartColors::default(),
        );
        // Time axis pinned to the union span over every loaded place, so the
        // percent-based zoom sync of the connect group aligns real instants.
        assert_eq!(opt["xAxis"]["type"], "time");
        assert_eq!(opt["xAxis"]["min"], ms(2026, 7, 9, 12));
        assert_eq!(opt["xAxis"]["max"], ms(2026, 7, 9, 15));
        assert!(opt["xAxis"]["data"].is_null());
        assert_eq!(opt["xAxis"]["axisLabel"]["hideOverlap"], true);
        // Exactly one series, named after the location (the tooltip shows
        // it), with [ts, temp, emoji] points on Berlin's own UTC+2 offset.
        let series = opt["series"].as_array().unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0]["name"], "Berlin");
        assert_eq!(
            series[0]["data"],
            serde_json::json!([
                [ms(2026, 7, 9, 12), 20.0, crate::weather_emoji(0)],
                [ms(2026, 7, 9, 13), 21.0, crate::weather_emoji(2)],
            ])
        );
        assert!(opt["legend"].is_null());
        // Now-marker at the real instant.
        assert_eq!(series[0]["markLine"]["data"][0]["xAxis"], now_ms);
        // Alternating viewer-local day bands ride the series.
        let bands = &series[0]["markArea"];
        assert_eq!(bands["itemStyle"]["opacity"], 0.08);
        assert_eq!(bands["data"][0][0]["xAxis"], ms(2026, 7, 9, 0));
        assert_eq!(bands["data"][0][1]["xAxis"], ms(2026, 7, 9, 15));
        // y pinned to the page-wide range (both locations): (19, 32).
        assert_eq!(opt["yAxis"]["min"], 19.0);
        assert_eq!(opt["yAxis"]["max"], 32.0);
        // Wheel zoom + drag pan on; no start/end in the option (the reset
        // window is dispatched by ChartCanvas, not merged on re-renders).
        assert_eq!(opt["dataZoom"][0]["zoomOnMouseWheel"], true);
        assert_eq!(opt["dataZoom"][0]["moveOnMouseMove"], true);
        assert!(opt["dataZoom"][0]["start"].is_null());
        assert!(opt["dataZoom"][0]["end"].is_null());
        assert!(opt["toolbox"].is_null());
    }

    #[test]
    fn combined_option_aligns_each_place_by_real_instant() {
        let all = two_locations();
        let now_ms = ms(2026, 7, 9, 13);
        let opt = combined_chart_option(
            &all,
            now_ms,
            0,
            false,
            &crate::echarts::ChartColors::default(),
        );
        // Same pinned time axis as the split charts.
        assert_eq!(opt["xAxis"]["type"], "time");
        assert_eq!(opt["xAxis"]["min"], ms(2026, 7, 9, 12));
        assert_eq!(opt["xAxis"]["max"], ms(2026, 7, 9, 15));
        let series = opt["series"].as_array().unwrap();
        assert_eq!(series.len(), 2);
        // Each place plots on its OWN offset: the same local 14:00 lands an
        // hour apart in real time (the bug the time axis fixes).
        assert_eq!(series[0]["name"], "Berlin");
        assert_eq!(series[0]["data"][0][0], ms(2026, 7, 9, 12));
        assert_eq!(series[1]["name"], "Nijar");
        assert_eq!(series[1]["data"][0][0], ms(2026, 7, 9, 14));
        // Distinct palette colors.
        for s in series {
            assert!(s["color"].as_str().unwrap().starts_with('#'));
        }
        assert_ne!(series[0]["color"], series[1]["color"]);
        // Legend names the locations; tooltip compares them natively.
        assert_eq!(
            opt["legend"]["data"],
            serde_json::json!(["Berlin", "Nijar"])
        );
        // Now-marker and day bands ride the first series only.
        assert_eq!(series[0]["markLine"]["data"][0]["xAxis"], now_ms);
        assert!(series[1]["markLine"].is_null());
        assert_eq!(series[0]["markArea"]["itemStyle"]["opacity"], 0.08);
        assert!(series[1]["markArea"].is_null());
        // Same pinned page-wide y-range as split view.
        assert_eq!(opt["yAxis"]["min"], 19.0);
        assert_eq!(opt["yAxis"]["max"], 32.0);
    }

    #[test]
    fn combined_option_survives_empty_list() {
        // Transient retract-to-empty must not panic (wasm aborts on panic).
        let opt = combined_chart_option(
            &LoadedPlaces::new(),
            0,
            0,
            false,
            &crate::echarts::ChartColors::default(),
        );
        assert!(opt["series"].is_null());
    }

    #[test]
    fn split_option_folds_own_unpublished_data_into_both_ranges() {
        // Own data not in the shared list still shapes the y-range and the
        // axis span (its publish may not have landed yet).
        let all = two_locations();
        let own = LoadedPlace {
            name: "Reykjavik".to_string(),
            hourly: vec![hour(2026, 7, 9, 16, 10.0, 0)],
            now_index: 0,
            utc_offset_seconds: 0,
        };
        let opt = weather_chart_option(
            &own,
            &all,
            ms(2026, 7, 9, 13),
            0,
            false,
            &crate::echarts::ChartColors::default(),
        );
        assert_eq!(opt["yAxis"]["min"], 9.0);
        assert_eq!(opt["yAxis"]["max"], 32.0);
        assert_eq!(opt["xAxis"]["max"], ms(2026, 7, 9, 16));
    }
}
