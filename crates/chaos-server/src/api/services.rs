//! Health and monitored-service endpoints.

use axum::Json;
use axum::extract::{Path, State};
use chaos_domain::{
    HealthResponse, ServiceActionRequest, ServiceDef, ServiceStatus, ServiceWithStatus,
};

use crate::api::ApiError;
use crate::state::AppState;

pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        fahrenheit: Some(locale_fahrenheit()),
    })
}

/// Whether the host locale measures in Fahrenheit, from the usual POSIX
/// precedence. Browsers can't read the system locale, so this is their
/// units default (mirrors `fahrenheit_locale` in chaos-ui).
fn locale_fahrenheit() -> bool {
    // The short list of regions still on °F (US and its close orbit).
    const FAHRENHEIT_REGIONS: [&str; 8] = ["US", "BS", "BZ", "KY", "LR", "PW", "FM", "MH"];
    ["LC_MEASUREMENT", "LC_ALL", "LANG"]
        .iter()
        .find_map(|var| std::env::var(var).ok().filter(|v| !v.trim().is_empty()))
        // "fr_FR.UTF-8" → region "FR"
        .and_then(|locale| {
            locale
                .split('.')
                .next()?
                .rsplit(['_', '-'])
                .next()
                .map(str::to_ascii_uppercase)
        })
        .is_some_and(|region| FAHRENHEIT_REGIONS.contains(&region.as_str()))
}

pub async fn services(State(state): State<AppState>) -> Json<Vec<ServiceWithStatus>> {
    let statuses = state.statuses.read().await;
    let list = state
        .config
        .services
        .iter()
        .map(|def| ServiceWithStatus {
            def: def.clone(),
            status: statuses
                .get(&def.id)
                .cloned()
                .unwrap_or_else(ServiceStatus::unknown),
        })
        .collect();
    Json(list)
}

/// Start/stop an on-demand service's systemd unit, then re-check and return
/// its fresh status. Only services configured with a `unit` are actionable —
/// the unit name never comes from the client, and polkit further restricts
/// what the chaos user may touch (`systemdControl.units`).
pub async fn service_systemd(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ServiceActionRequest>,
) -> Result<Json<ServiceWithStatus>, ApiError> {
    let service = on_demand_service(&state.config.services, &id)?;

    tracing::info!(service = id, verb = req.action.verb(), "service action");
    crate::widgets::systemd::act(service.unit.as_deref().expect("checked"), req.action)
        .await
        .map_err(ApiError::BadGateway)?;

    // Re-check immediately so the tile flips to starting/paused without
    // waiting for the next monitor sweep.
    let client = crate::monitor::client(&state.config.monitor);
    let status = crate::monitor::check(&client, service).await;
    state
        .statuses
        .write()
        .await
        .insert(service.id.clone(), status.clone());

    Ok(Json(ServiceWithStatus {
        def: service.clone(),
        status,
    }))
}

fn on_demand_service<'a>(services: &'a [ServiceDef], id: &str) -> Result<&'a ServiceDef, ApiError> {
    let service = services
        .iter()
        .find(|s| s.id == id)
        .ok_or(ApiError::NotFound)?;
    if service.unit.is_none() {
        return Err(ApiError::Unprocessable(format!(
            "service {id:?} has no systemd unit configured"
        )));
    }
    Ok(service)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_actions_require_a_configured_unit() {
        let service = |id: &str, unit: Option<&str>| ServiceDef {
            id: id.into(),
            title: id.into(),
            url: "http://zeus:1234".parse().unwrap(),
            icon: None,
            check_url: None,
            unit: unit.map(Into::into),
        };
        let services = [
            service("jellyfin", None),
            service("stirling-pdf", Some("stirling-pdf.service")),
        ];

        assert!(matches!(
            on_demand_service(&services, "nope"),
            Err(ApiError::NotFound)
        ));
        assert!(matches!(
            on_demand_service(&services, "jellyfin"),
            Err(ApiError::Unprocessable(_))
        ));
        assert_eq!(
            on_demand_service(&services, "stirling-pdf").unwrap().id,
            "stirling-pdf"
        );
    }
}
