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
mod static_assets;
mod views;
mod widgets;

use axum::Router;
use axum::routing::{get, post, put};
use tower_http::cors::CorsLayer;
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
        .route("/posts/{source}", get(widgets::posts_list))
        .route("/posts/{source}/views", get(views::views_map))
        .route("/posts/views", post(views::record_views))
        .route("/analytics/events", post(views::record_events))
        .route("/posts/{source}/{id}/comments", get(widgets::post_thread))
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
        app = app.merge(static_assets::router(dir));
    }

    app
        // The desktop app runs on a tauri:// origin, and LAN clients hit the
        // server cross-origin. Every route but /health and /auth/login now
        // requires a signed-in user, so a permissive CORS policy only exposes
        // what the caller could already reach with its own credentials.
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::db::Db;

    /// Overlapping routes panic at construction (matchit), not at request
    /// time — so building the router is itself the regression guard for the
    /// static `/posts/views` sibling of `/posts/{source}`.
    #[tokio::test]
    async fn router_builds_without_route_conflicts() {
        let db = Db::in_memory().await.unwrap();
        let state = AppState::new(Config::default(), db).unwrap();
        let _ = router(state);
    }

    /// Every route must require a signed-in user except the three that cannot:
    /// `/health` (liveness + the auth advertisement the apps read before they
    /// have a token), `/auth/login` (how you get one), and `/icons/{spec}`
    /// (referenced from `<img src>`, which cannot carry an Authorization
    /// header — see the note on the handler).
    ///
    /// Hitting each route unauthenticated and asserting 401 is what makes this
    /// a real guard: a handler that forgets `AuthUser` fails here instead of
    /// silently serving data on the public domain.
    #[tokio::test]
    async fn every_route_requires_auth_except_the_allowlist() {
        use axum::body::Body;
        use axum::http::{Method, Request, StatusCode};
        use tower::ServiceExt;

        const ALLOWLISTED: [(&str, Method); 3] = [
            ("/api/v1/health", Method::GET),
            ("/api/v1/auth/login", Method::POST),
            ("/api/v1/icons/si:github", Method::GET),
        ];

        // (path, method) for every route in the router. Keep in sync with
        // `router()` — a new route without an entry here is a review miss.
        let routes: Vec<(&str, Method)> = vec![
            ("/api/v1/health", Method::GET),
            ("/api/v1/auth/login", Method::POST),
            ("/api/v1/auth/logout", Method::POST),
            ("/api/v1/auth/me", Method::GET),
            ("/api/v1/calendars", Method::GET),
            ("/api/v1/calendars", Method::POST),
            (
                "/api/v1/calendars/00000000-0000-0000-0000-000000000000",
                Method::PUT,
            ),
            (
                "/api/v1/calendars/00000000-0000-0000-0000-000000000000",
                Method::DELETE,
            ),
            ("/api/v1/calendar/events", Method::GET),
            ("/api/v1/calendar/refresh", Method::POST),
            ("/api/v1/events", Method::POST),
            (
                "/api/v1/events/00000000-0000-0000-0000-000000000000",
                Method::PUT,
            ),
            (
                "/api/v1/events/00000000-0000-0000-0000-000000000000",
                Method::DELETE,
            ),
            ("/api/v1/services", Method::GET),
            ("/api/v1/services/x/systemd", Method::POST),
            ("/api/v1/dashboard", Method::GET),
            ("/api/v1/widgets/x", Method::GET),
            ("/api/v1/widgets/x/systemd", Method::POST),
            ("/api/v1/posts/hackernews", Method::GET),
            ("/api/v1/posts/hackernews/views", Method::GET),
            ("/api/v1/posts/views", Method::POST),
            ("/api/v1/analytics/events", Method::POST),
            ("/api/v1/posts/hackernews/1/comments", Method::GET),
            ("/api/v1/home/sensors", Method::GET),
            ("/api/v1/home/lights", Method::GET),
            ("/api/v1/home/lights/x", Method::POST),
            ("/api/v1/home/temperature", Method::GET),
            ("/api/v1/icons/si:github", Method::GET),
            ("/api/v1/links", Method::GET),
            ("/api/v1/links", Method::POST),
            (
                "/api/v1/links/00000000-0000-0000-0000-000000000000",
                Method::GET,
            ),
            (
                "/api/v1/links/00000000-0000-0000-0000-000000000000",
                Method::PUT,
            ),
            (
                "/api/v1/links/00000000-0000-0000-0000-000000000000",
                Method::DELETE,
            ),
            (
                "/api/v1/links/00000000-0000-0000-0000-000000000000/archive",
                Method::GET,
            ),
            (
                "/api/v1/links/00000000-0000-0000-0000-000000000000/archive",
                Method::POST,
            ),
            ("/api/v1/collections", Method::GET),
            ("/api/v1/collections", Method::POST),
            (
                "/api/v1/collections/00000000-0000-0000-0000-000000000000",
                Method::PUT,
            ),
            (
                "/api/v1/collections/00000000-0000-0000-0000-000000000000",
                Method::DELETE,
            ),
            ("/api/v1/tags", Method::GET),
            ("/api/v1/search", Method::GET),
        ];

        for (path, method) in routes {
            let db = Db::in_memory().await.unwrap();
            let state = AppState::new(Config::default(), db).unwrap();
            let request = Request::builder()
                .method(method.clone())
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap();
            let status = router(state)
                .oneshot(request)
                .await
                .expect("infallible")
                .status();

            if ALLOWLISTED.contains(&(path, method.clone())) {
                assert_ne!(
                    status,
                    StatusCode::UNAUTHORIZED,
                    "{method} {path} is allowlisted but rejected the request"
                );
            } else {
                assert_eq!(
                    status,
                    StatusCode::UNAUTHORIZED,
                    "{method} {path} served an unauthenticated request"
                );
            }
        }
    }
}
