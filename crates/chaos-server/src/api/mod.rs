//! HTTP API (`/api/v1`) and static frontend serving.

mod auth;
mod calendar;
mod collections;
mod error;
mod home;
mod icons;
mod links;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chaos_domain::{
    DashboardLayout, HealthResponse, ServiceActionRequest, ServiceDef, ServiceStatus,
    ServiceWithStatus, SystemdActionRequest, WidgetData,
};
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

pub use error::ApiError;

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me))
        .route("/calendars", get(calendar::list).post(calendar::create))
        .route(
            "/calendars/{id}",
            put(calendar::update).delete(calendar::delete),
        )
        .route("/calendar/events", get(calendar::events))
        .route("/calendar/refresh", post(calendar::refresh))
        .route("/events", post(calendar::create_event))
        .route(
            "/events/{id}",
            put(calendar::update_event).delete(calendar::delete_event),
        )
        .route("/apps", get(apps))
        .route("/services", get(services))
        .route("/services/{id}/systemd", post(service_systemd))
        .route("/dashboard", get(dashboard))
        .route("/widgets/{id}", get(widget_data))
        .route("/widgets/{id}/systemd", post(widget_systemd))
        .route("/weather", get(weather))
        .route("/home/sensors", get(home::sensors))
        .route("/home/lights", get(home::lights))
        .route("/home/lights/{id}", post(home::set_light))
        .route("/home/temperature", get(home::temperature))
        .route("/icons/{spec}", get(icons::icon))
        .route("/links", get(links::list).post(links::create))
        .route(
            "/links/{id}",
            get(links::get_one).put(links::update).delete(links::delete),
        )
        .route(
            "/links/{id}/archive",
            get(links::serve_archive).post(links::rearchive),
        )
        .route(
            "/collections",
            get(collections::list).post(collections::create),
        )
        .route(
            "/collections/{id}",
            put(collections::update).delete(collections::delete),
        )
        .route("/tags", get(links::tags))
        .with_state(state.clone());

    let mut app = Router::new().nest("/api/v1", api);

    // Serve the built web frontend when configured (production mode). During
    // development trunk serves it instead and proxies /api here.
    if let Some(dir) = &state.config.static_dir {
        let index = dir.join("index.html");
        app = app.fallback_service(ServeDir::new(dir).fallback(ServeFile::new(index)));
    }

    app
        // The desktop app runs on a tauri:// origin, and LAN clients hit the
        // server cross-origin. The API is read-mostly and LAN-only for now;
        // revisit when auth lands (see ROADMAP).
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}

async fn health() -> Json<HealthResponse> {
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

async fn dashboard(State(state): State<AppState>) -> Json<DashboardLayout> {
    Json(state.widgets.layout.clone())
}

#[derive(serde::Deserialize)]
struct WidgetQuery {
    /// Device preference: weather widgets fetch this location instead of
    /// the configured one.
    location: Option<String>,
}

async fn widget_data(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<WidgetQuery>,
) -> Result<Json<WidgetData>, ApiError> {
    state
        .widgets
        .data(&id, query.location.as_deref())
        .await
        .map(Json)
        .map_err(Into::into)
}

/// Forecast for any location (the weather page), not tied to a widget
/// instance; without `location` the layout's weather widget place is used.
async fn weather(
    State(state): State<AppState>,
    Query(query): Query<WidgetQuery>,
) -> Result<Json<chaos_domain::WeatherData>, ApiError> {
    state
        .widgets
        .weather(query.location.as_deref())
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn widget_systemd(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SystemdActionRequest>,
) -> Result<Json<WidgetData>, ApiError> {
    state
        .widgets
        .systemd_action(&id, &req)
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn apps(State(state): State<AppState>) -> Json<Vec<chaos_domain::AppLink>> {
    Json(state.config.apps.clone())
}

/// Start/stop an on-demand service's systemd unit, then re-check and return
/// its fresh status. Only services configured with a `unit` are actionable —
/// the unit name never comes from the client, and polkit further restricts
/// what the chaos user may touch (`systemdControl.units`).
async fn service_systemd(
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

async fn services(State(state): State<AppState>) -> Json<Vec<ServiceWithStatus>> {
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
