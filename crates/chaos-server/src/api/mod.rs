//! HTTP API (`/api/v1`) and static frontend serving.

mod auth;
mod calendar;
mod collections;
mod error;
mod icons;
mod links;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chaos_domain::{
    DashboardLayout, HealthResponse, ServiceStatus, ServiceWithStatus, SystemdActionRequest,
    WidgetData,
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
        .route("/dashboard", get(dashboard))
        .route("/widgets/{id}", get(widget_data))
        .route("/widgets/{id}/systemd", post(widget_systemd))
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
    })
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
