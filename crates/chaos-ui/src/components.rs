use chaos_domain::{HealthState, ServiceWithStatus, SystemdAction};
use leptos::prelude::*;

/// Start/stop plumbing for on-demand services (those with a systemd unit):
/// a busy flag shared across the grid and the callback running the action,
/// keyed by service id. Tiles without a unit never show controls.
pub type ServiceControls = (RwSignal<bool>, Callback<(String, SystemdAction)>);

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
pub fn ServiceGrid(services: Vec<ServiceWithStatus>, controls: ServiceControls) -> impl IntoView {
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
                .map(|service| view! { <ServiceCard service controls/> })
                .collect_view()}
        </div>
    }
    .into_any()
}

#[component]
fn ServiceCard(service: ServiceWithStatus, controls: ServiceControls) -> impl IntoView {
    let state = service.status.state;
    let (dot_class, state_label) = match state {
        HealthState::Up => ("dot up", "up"),
        HealthState::Degraded => ("dot degraded", "degraded"),
        HealthState::Down => ("dot down", "down"),
        HealthState::Paused => ("dot paused", "paused"),
        HealthState::Starting => ("dot degraded", "starting…"),
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

    // On-demand services carry a start/stop button on the tile itself; the
    // click must not follow the card's link.
    let (busy, run) = controls;
    let action = service.def.unit.is_some().then(|| {
        let id = service.def.id.clone();
        let (label, title, action) = match state {
            HealthState::Paused | HealthState::Down | HealthState::Unknown => {
                ("▶", "Start", SystemdAction::Start)
            }
            _ => ("■", "Stop", SystemdAction::Stop),
        };
        view! {
            <button
                class="unit-btn service-btn"
                title=title
                disabled=move || busy.get()
                on:click=move |ev| {
                    ev.prevent_default();
                    ev.stop_propagation();
                    run.run((id.clone(), action));
                }
            >
                {label}
            </button>
        }
    });

    view! {
        <a class="service-card" href=service.def.url.to_string() target="_blank" rel="noreferrer">
            {icon
                .map(|url| {
                    view! { <img class="service-icon" src=url.to_string() loading="lazy" alt=""/> }
                })}
            <span class="service-title">{service.def.title}</span>
            <span class="service-latency muted">{state_label_detail(state, latency)}</span>
            {action}
            <span class=dot_class title=state_label></span>
        </a>
    }
}

/// The small grey detail on a tile: latency when the service answers,
/// otherwise the lifecycle state (paused/starting) so an on-demand service
/// at rest doesn't just look broken.
fn state_label_detail(state: HealthState, latency: String) -> String {
    match state {
        HealthState::Paused => "paused".into(),
        HealthState::Starting => "starting…".into(),
        _ => latency,
    }
}
