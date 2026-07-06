use chaos_ui::{App, AppConfig};
use leptos::prelude::*;
use url::Url;

/// Where the chaos API lives, resolved in order:
///
/// 1. `window.CHAOS_API_BASE` — set by a hosting shell (the Tauri shell
///    injects it; a reverse proxy could inline a `<script>`).
/// 2. `localStorage["chaos-api-base"]` — user override written by the
///    connect screen (the path on Android), survives reloads.
/// 3. The page origin, when it can actually be the server — the
///    served-by-chaos-server case (and trunk's dev proxy). Tauri's own
///    origins (`tauri://localhost`, `http://tauri.localhost`) are the app
///    bundle, never the API.
/// 4. The default local server.
fn resolve() -> (Url, bool) {
    let fallback = Url::parse("http://127.0.0.1:4600").expect("valid fallback url");
    let Some(window) = web_sys::window() else {
        return (fallback, false);
    };

    let injected = js_sys::Reflect::get(&window, &"CHAOS_API_BASE".into())
        .ok()
        .and_then(|v| v.as_string());
    let stored = window
        .local_storage()
        .ok()
        .flatten()
        .and_then(|s| s.get_item("chaos-api-base").ok().flatten());
    if let Some(url) = [injected, stored]
        .into_iter()
        .flatten()
        .find_map(|raw| Url::parse(&raw).ok())
    {
        return (url, true);
    }

    match window.location().origin().ok().map(|o| Url::parse(&o)) {
        Some(Ok(url))
            if (url.scheme() == "http" || url.scheme() == "https")
                && url.host_str() != Some("tauri.localhost") =>
        {
            (url, false)
        }
        _ => (fallback, true),
    }
}

fn main() {
    console_error_panic_hook::set_once();
    // Cross-origin API (shell or override): the session cookie won't flow,
    // so the client keeps the bearer token in localStorage instead.
    let (api_base, cross_origin) = resolve();
    let config = AppConfig {
        api_base,
        persist_token: cross_origin,
    };
    mount_to_body(move || view! { <App config=config.clone()/> });
}
