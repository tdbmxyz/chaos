use chaos_ui::{App, AppConfig};
use leptos::prelude::*;
use url::Url;

/// Where the chaos API lives, resolved in order:
///
/// 1. `window.CHAOS_API_BASE` — optional hosting-page configuration.
/// 2. `localStorage["chaos-api-base"]` — user override from Settings.
/// 3. The page origin (chaos-server in production, trunk's dev proxy locally).
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
        Some(Ok(url)) if url.scheme() == "http" || url.scheme() == "https" => (url, false),
        _ => (fallback, true),
    }
}

fn main() {
    console_error_panic_hook::set_once();
    // Cross-origin API override: the session cookie won't flow,
    // so the client keeps the bearer token in localStorage instead.
    let (api_base, cross_origin) = resolve();
    let config = AppConfig {
        api_base,
        persist_token: cross_origin,
    };
    mount_to_body(move || view! { <App config=config.clone()/> });
    // The boot skeleton (index.html) paints while the wasm loads; mount_to_body
    // appends the app beside it rather than replacing it, so drop it now.
    if let Some(node) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("chaos-boot"))
    {
        node.remove();
    }
}
