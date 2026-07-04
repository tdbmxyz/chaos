//! Shared Leptos UI: the same `App` component is mounted by the web bundle
//! (trunk) and rendered inside the Tauri webview. Anything platform-specific
//! (like where the API lives) is injected from the outside via [`AppConfig`].

mod components;
mod pages;

use chaos_client::ChaosClient;
use leptos::prelude::*;
use leptos_router::components::{A, Route, Router, Routes};
use leptos_router::path;
use url::Url;

/// Platform-provided configuration, put into the reactive context so every
/// component can reach the API client without prop-drilling.
#[derive(Clone)]
pub struct AppConfig {
    pub api_base: Url,
}

pub fn use_client() -> ChaosClient {
    let config = use_context::<AppConfig>().expect("AppConfig provided by the shell");
    ChaosClient::new(config.api_base)
}

#[component]
pub fn App(config: AppConfig) -> impl IntoView {
    provide_context(config);

    view! {
        <Router>
            <nav class="topbar">
                <span class="brand">"chaos"</span>
                <A href="/">"Dashboard"</A>
                <A href="/links">"Links"</A>
            </nav>
            <main>
                <Routes fallback=|| view! { <p class="muted">"Page not found"</p> }>
                    <Route path=path!("/") view=pages::Dashboard/>
                    <Route path=path!("/links") view=pages::Links/>
                </Routes>
            </main>
        </Router>
    }
}
