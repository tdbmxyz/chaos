use leptos::prelude::*;

use crate::{THEMES, WEATHER_LOCATION_KEY, WEATHER_UNITS_KEY, pref, set_pref, use_theme};

/// Device-local preferences: theme, weather location/units. Server URL and
/// account management are candidates once they exist.
#[component]
pub fn Settings() -> impl IntoView {
    let theme = use_theme().0;

    let location = RwSignal::new(pref(WEATHER_LOCATION_KEY).unwrap_or_default());
    let units = RwSignal::new(pref(WEATHER_UNITS_KEY).unwrap_or_else(|| "celsius".into()));

    view! {
        <div class="settings-page">
            <h2>"Settings"</h2>
            <p class="muted">"Stored on this device."</p>

            <h3>"Weather"</h3>
            <div class="settings-weather">
                <label>
                    "Location"
                    <input
                        type="text"
                        placeholder="Server default — e.g. Lyon or Osaka, JP"
                        prop:value=location
                        on:input=move |ev| location.set(event_target_value(&ev))
                        on:change=move |_| set_pref(
                            WEATHER_LOCATION_KEY,
                            &location.get_untracked(),
                        )
                    />
                </label>
                <label>
                    "Units"
                    <select on:change=move |ev| {
                        let value = event_target_value(&ev);
                        set_pref(WEATHER_UNITS_KEY, &value);
                        units.set(value);
                    }>
                        <option value="celsius" selected=move || units.get() == "celsius">
                            "Celsius (°C)"
                        </option>
                        <option value="fahrenheit" selected=move || units.get() == "fahrenheit">
                            "Fahrenheit (°F)"
                        </option>
                    </select>
                </label>
                <p class="muted settings-hint">
                    "Leave the location empty to use the server's configured place."
                </p>
            </div>

            <h3>"Theme"</h3>
            <div class="theme-options">
                {THEMES
                    .iter()
                    .map(|t| {
                        let id = t.id;
                        view! {
                            <label class="theme-option" class:active=move || theme.get() == id>
                                <input
                                    type="radio"
                                    name="theme"
                                    value=id
                                    checked=move || theme.get() == id
                                    on:change=move |_| theme.set(id.to_string())
                                />
                                <span>
                                    <span class="theme-option-name">{t.name}</span>
                                    <br/>
                                    <span class="theme-option-desc muted">{t.description}</span>
                                </span>
                                <span class="theme-swatches">
                                    {t
                                        .swatches
                                        .iter()
                                        .map(|c| {
                                            view! { <span style=format!("background: {c}")></span> }
                                        })
                                        .collect_view()}
                                </span>
                            </label>
                        }
                    })
                    .collect_view()}
            </div>
        </div>
    }
}
