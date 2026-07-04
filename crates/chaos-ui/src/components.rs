use chaos_domain::{HealthState, ServiceWithStatus};
use leptos::prelude::*;

/// Centered dialog over a click-to-close backdrop.
#[component]
pub fn Modal(
    title: String,
    #[prop(into)] on_close: Callback<()>,
    children: Children,
) -> impl IntoView {
    view! {
        <div class="modal-backdrop" on:click=move |_| on_close.run(())>
            <div class="modal" on:click=|ev| ev.stop_propagation()>
                <div class="modal-head">
                    <h3>{title}</h3>
                    <button class="modal-close" on:click=move |_| on_close.run(())>
                        "✕"
                    </button>
                </div>
                {children()}
            </div>
        </div>
    }
}

#[component]
pub fn ServiceGrid(services: Vec<ServiceWithStatus>) -> impl IntoView {
    if services.is_empty() {
        return view! {
            <p class="muted">"No services configured. Add some to chaos.toml."</p>
        }
        .into_any();
    }

    view! {
        <div class="service-grid">
            {services
                .into_iter()
                .map(|service| view! { <ServiceCard service/> })
                .collect_view()}
        </div>
    }
    .into_any()
}

#[component]
fn ServiceCard(service: ServiceWithStatus) -> impl IntoView {
    let (dot_class, state_label) = match service.status.state {
        HealthState::Up => ("dot up", "up"),
        HealthState::Degraded => ("dot degraded", "degraded"),
        HealthState::Down => ("dot down", "down"),
        HealthState::Unknown => ("dot unknown", "…"),
    };
    let latency = service
        .status
        .latency_ms
        .map(|ms| format!("{ms} ms"))
        .unwrap_or_default();
    let icon = service
        .def
        .icon
        .as_deref()
        .and_then(|spec| crate::use_client().icon_url(spec));

    view! {
        <a class="service-card" href=service.def.url.to_string() target="_blank" rel="noreferrer">
            {icon
                .map(|url| {
                    view! { <img class="service-icon" src=url.to_string() loading="lazy" alt=""/> }
                })}
            <span class="service-title">{service.def.title}</span>
            <span class="service-latency muted">{latency}</span>
            <span class=dot_class title=state_label></span>
        </a>
    }
}
