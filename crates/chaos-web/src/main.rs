use chaos_ui::{App, AppConfig};
use leptos::prelude::*;
use url::Url;

/// Where the chaos API lives.
///
/// - Served by chaos-server or trunk (which proxies /api): same origin.
/// - Inside the Tauri webview the origin is tauri://localhost, which cannot
///   host the API; fall back to the default local server. A proper server
///   picker for the desktop app is tracked in the roadmap.
fn api_base() -> Url {
    let fallback = Url::parse("http://127.0.0.1:4600").expect("valid fallback url");

    let Some(origin) = web_sys::window().and_then(|w| w.location().origin().ok()) else {
        return fallback;
    };
    match Url::parse(&origin) {
        Ok(url) if url.scheme() == "http" || url.scheme() == "https" => url,
        _ => fallback,
    }
}

fn main() {
    console_error_panic_hook::set_once();
    let config = AppConfig {
        api_base: api_base(),
    };
    mount_to_body(move || view! { <App config=config.clone()/> });
}
