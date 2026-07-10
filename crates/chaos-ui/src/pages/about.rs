//! Version check: the web bundle's compiled-in version against the server's
//! (from /health), to verify an installation is complete and in sync.

use leptos::prelude::*;

use crate::use_client;

const WEB_VERSION: &str = env!("CARGO_PKG_VERSION");

#[component]
pub fn AboutPage() -> impl IntoView {
    let health = LocalResource::new({
        let client = use_client();
        move || {
            let client = client.clone();
            async move { client.health().await }
        }
    });

    view! {
        <div class="about-page">
            <h2>"About"</h2>
            <dl class="about-versions">
                <dt>"Web"</dt>
                <dd>{WEB_VERSION}</dd>
                <dt>"Server"</dt>
                <dd>
                    {move || match health.get() {
                        None => view! { <span class="muted">"…"</span> }.into_any(),
                        Some(Err(_)) => view! { <span class="error">"unreachable"</span> }.into_any(),
                        Some(Ok(h)) => view! { <span>{h.version}</span> }.into_any(),
                    }}
                </dd>
            </dl>
            {move || match health.get() {
                Some(Ok(h)) if h.version == WEB_VERSION => {
                    view! { <p class="about-sync ok">"in sync ✓"</p> }.into_any()
                }
                Some(Ok(_)) => {
                    view! { <p class="about-sync warn">"version mismatch ⚠ — redeploy or refresh"</p> }.into_any()
                }
                _ => view! { <span></span> }.into_any(),
            }}
        </div>
    }
}
