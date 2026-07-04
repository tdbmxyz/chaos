use leptos::prelude::*;

use crate::components::ServiceGrid;
use crate::use_client;

#[component]
pub fn Dashboard() -> impl IntoView {
    let client = use_client();
    let services = LocalResource::new(move || {
        let client = client.clone();
        async move { client.services().await }
    });

    view! {
        <section>
            <h2>"Services"</h2>
            {move || match services.get() {
                None => view! { <p class="muted">"Checking services…"</p> }.into_any(),
                Some(Ok(list)) => view! { <ServiceGrid services=list/> }.into_any(),
                Some(Err(err)) => {
                    view! { <p class="error">"Could not reach chaos server: " {err.to_string()}</p> }
                        .into_any()
                }
            }}
        </section>
    }
}

#[component]
pub fn Links() -> impl IntoView {
    view! {
        <section>
            <h2>"Links"</h2>
            <p class="muted">"Link management lands in Phase 2 (see ROADMAP.md)."</p>
        </section>
    }
}
