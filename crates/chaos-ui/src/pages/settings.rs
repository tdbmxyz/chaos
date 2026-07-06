use leptos::prelude::*;

use crate::{THEMES, use_theme};

/// Device-local preferences. Just the theme today; server URL and account
/// management are candidates once they exist.
#[component]
pub fn Settings() -> impl IntoView {
    let theme = use_theme().0;

    view! {
        <div class="settings-page">
            <h2>"Settings"</h2>
            <p class="muted">"Stored on this device."</p>
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
