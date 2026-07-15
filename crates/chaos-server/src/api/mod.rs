//! HTTP API (`/api/v1`) and static frontend serving.

mod auth;
mod calendar;
mod collections;
mod error;
mod home;
mod icons;
mod links;
mod search;
mod services;
mod widgets;

use axum::Router;
use axum::routing::{get, post, put};
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

pub use error::ApiError;

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/health", get(services::health))
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
        .route("/services", get(services::services))
        .route("/services/{id}/systemd", post(services::service_systemd))
        .route("/dashboard", get(widgets::dashboard))
        .route("/widgets/{id}", get(widgets::widget_data))
        .route("/widgets/{id}/systemd", post(widgets::widget_systemd))
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
        .route("/search", get(search::search))
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
