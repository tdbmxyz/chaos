use leptos::prelude::*;
use url::Url;

use crate::{THEMES, WEATHER_LOCATION_KEY, WEATHER_UNITS_KEY, pref, set_pref, use_theme};

/// Device-local preferences: server address, theme, weather location/units.
#[component]
pub fn Settings() -> impl IntoView {
    let theme = use_theme().0;

    let location = RwSignal::new(pref(WEATHER_LOCATION_KEY).unwrap_or_default());
    // Empty = follow the system locale; "celsius"/"fahrenheit" override it.
    let units = RwSignal::new(pref(WEATHER_UNITS_KEY).unwrap_or_default());
    let locale_units = if crate::default_fahrenheit() {
        "System locale (°F)"
    } else {
        "System locale (°C)"
    };

    // Server address: applying stores the override and reloads the app, so
    // everything (client, session, gate) starts over against the new server.
    let server = RwSignal::new(crate::use_client().base().to_string());
    let server_error = RwSignal::new(false);
    let has_override = crate::api_base_override().is_some();
    let connect = move |_| {
        let value = server.get_untracked().trim().to_string();
        if Url::parse(&value).is_err() {
            server_error.set(true);
            return;
        }
        crate::set_api_base_override(Some(&value));
    };

    view! {
        <div class="settings-page">
            <h2>"Settings"</h2>
            <p class="muted">"Stored on this device."</p>

            <h3>"Server"</h3>
            <div class="settings-server">
                <input
                    type="url"
                    prop:value=server
                    on:input=move |ev| {
                        server_error.set(false);
                        server.set(event_target_value(&ev));
                    }
                />
                <button class="primary" on:click=connect>
                    "Connect"
                </button>
                {has_override
                    .then(|| {
                        view! {
                            <button
                                title="Forget the override and use this device's default"
                                on:click=move |_| crate::set_api_base_override(None)
                            >
                                "Use default"
                            </button>
                        }
                    })}
                {move || {
                    server_error
                        .get()
                        .then(|| {
                            view! {
                                <p class="error settings-hint">
                                    "Not a valid URL — expected something like http://zeus:4600."
                                </p>
                            }
                        })
                }}
                <p class="muted settings-hint">
                    "Connecting reloads the app against the chosen server."
                </p>
            </div>

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
                        <option value="" selected=move || units.get().is_empty()>
                            {locale_units}
                        </option>
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
