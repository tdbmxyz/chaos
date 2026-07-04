//! HTTP API (`/api/v1`) and static frontend serving.

mod collections;
mod error;
mod links;

use axum::extract::State;
use axum::routing::{get, put};
use axum::{Json, Router};
use chaos_domain::{HealthResponse, ServiceStatus, ServiceWithStatus};
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

pub use error::ApiError;

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/services", get(services))
        .route("/links", get(links::list).post(links::create))
        .route(
            "/links/{id}",
            get(links::get_one).put(links::update).delete(links::delete),
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
