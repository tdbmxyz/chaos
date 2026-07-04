use chaos_domain::{HealthState, ServiceWithStatus};
use leptos::prelude::*;

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

    view! {
        <a class="service-card" href=service.def.url.to_string() target="_blank" rel="noreferrer">
            <span class=dot_class title=state_label></span>
            <span class="service-title">{service.def.title}</span>
            <span class="service-latency muted">{latency}</span>
        </a>
    }
}
