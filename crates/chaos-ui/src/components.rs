use chaos_domain::{HealthState, ServiceWithStatus, SystemdAction};
use leptos::prelude::*;

/// An icon `<img>` that degrades to a letter tile when the image fails to
/// load (icon source has no such slug, upstream 404, offline). The tile
/// shows the title's first letter on a background picked deterministically
/// from the title, so a service keeps its color across reloads.
#[component]
pub fn IconOrLetter(
    /// Resolved icon URL (already through `icon_url`); None renders the tile.
    url: Option<String>,
    title: String,
    /// CSS class of the img/tile, e.g. "service-icon" or "bookmark-icon".
    class: &'static str,
) -> impl IntoView {
    let failed = RwSignal::new(false);
    let letter = title
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    let color = TILE_PALETTE[hash_str(&title) as usize % TILE_PALETTE.len()];
    view! {
        {move || match (url.clone(), failed.get()) {
            (Some(url), false) => {
                view! {
                    <img
                        class=class
                        src=url
                        loading="lazy"
                        alt=""
                        on:error=move |_| failed.set(true)
                    />
                }
                    .into_any()
            }
            _ => {
                view! {
                    <span class=format!("{class} icon-letter") style:background=color>
                        {letter.clone()}
                    </span>
                }
                    .into_any()
            }
        }}
    }
}

const TILE_PALETTE: [&str; 6] = [
    "#5b6ee1", "#b45bcf", "#3f9d6b", "#c2703d", "#4f93b8", "#b8564f",
];

fn hash_str(s: &str) -> u32 {
    // FNV-1a, tiny and deterministic — only picks a palette slot.
    s.bytes()
        .fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619))
}

#[cfg(test)]
mod tests {
    #[test]
    fn tile_color_is_deterministic() {
        let a = super::hash_str("llamacpp");
        assert_eq!(a, super::hash_str("llamacpp"));
        assert_ne!(super::hash_str("a"), super::hash_str("b"));
    }
}

/// Start/stop plumbing for on-demand services (those with a systemd unit):
/// a busy flag shared across the grid and the callback running the action,
/// keyed by service id. Tiles without a unit never show controls.
pub type ServiceControls = (RwSignal<bool>, Callback<(String, SystemdAction)>);

/// Centered dialog over a click-to-close backdrop. Every dialog in the app
/// goes through this component: announced as a modal dialog, focused on
/// open, closed by Escape. Not a full WAI-ARIA dialog — there is no focus
/// trap and no focus return to the trigger; accepted for this app's
/// single-modal pages.
#[component]
pub fn Modal(
    title: String,
    #[prop(into)] on_close: Callback<()>,
    children: Children,
) -> impl IntoView {
    let dialog = NodeRef::<leptos::html::Div>::new();

    // Move focus into the dialog when it mounts so keyboard and
    // screen-reader users land inside it.
    Effect::new(move |_| {
        if let Some(el) = dialog.get() {
            let _ = el.focus();
        }
    });

    // Escape closes from anywhere while the dialog is up.
    let escape = window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() == "Escape" {
            on_close.run(());
        }
    });
    on_cleanup(move || escape.remove());

    let label = title.clone();
    view! {
        <div class="modal-backdrop" on:click=move |_| on_close.run(())>
            <div
                class="modal"
                role="dialog"
                aria-modal="true"
                aria-label=label
                tabindex="-1"
                node_ref=dialog
                on:click=|ev| ev.stop_propagation()
            >
                <div class="modal-head">
                    <h3>{title}</h3>
                    <button
                        class="modal-close"
                        aria-label="Close dialog"
                        on:click=move |_| on_close.run(())
                    >
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
            <For
                each=move || services.clone()
                key=|service| service.def.id.clone()
                children=move |service| view! { <ServiceCard service controls/> }
            />
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

    let title = service.def.title.clone();
    view! {
        <a class="service-card" href=service.def.url.to_string() target="_blank" rel="noreferrer">
            <IconOrLetter url=icon.map(|u| u.to_string()) title=title class="service-icon"/>
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
