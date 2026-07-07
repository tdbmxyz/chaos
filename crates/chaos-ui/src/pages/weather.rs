use chaos_domain::WeatherData;
use chrono::Timelike;
use leptos::prelude::*;

use crate::{WEATHER_LOCATION_KEY, use_client};

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
        <div class="hourly-strip">
            {weather
                .hourly
                .into_iter()
                .map(|hour| {
                    let day_break = hour.time.hour() == 0;
                    let label = if day_break {
                        hour.time.format("%a").to_string()
                    } else {
                        format!("{}h", hour.time.hour())
                    };
                    view! {
                        <div class="hour-cell" class:day-break=day_break>
                            <span class="muted hour-label">{label}</span>
                            <span>{crate::weather_emoji(hour.weather_code)}</span>
                            <span class="hour-temp">{temp(hour.temp_c)}</span>
                        </div>
                    }
                })
                .collect_view()}
        </div>
    }
}
