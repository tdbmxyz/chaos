//! `/api/v1/posts/{source}/views`, `/api/v1/posts/views` and
//! `/api/v1/analytics/events`: per-user post viewed-state and the generic
//! analytics event log. Everything requires a session.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chaos_domain::{RecordEventsRequest, RecordViewsRequest, Source, ViewedMap};

use crate::api::ApiError;
use crate::auth::AuthUser;
use crate::state::AppState;

/// The signed-in user's viewed-state for a source, keyed by post id.
pub async fn views_map(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Path(source): Path<String>,
) -> Result<Json<ViewedMap>, ApiError> {
    let src = Source::from_str(&source).ok_or(ApiError::NotFound)?;
    Ok(Json(state.db.viewed_map(user.id, src.as_str()).await?))
}

/// Record a batch of per-post engagement events for the signed-in user.
pub async fn record_views(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Json(req): Json<RecordViewsRequest>,
) -> Result<StatusCode, ApiError> {
    for e in &req.events {
        state
            .db
            .record_view(user.id, e.source.as_str(), &e.post_id, e.event, e.at)
            .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Record a batch of generic analytics events for the signed-in user.
pub async fn record_events(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Json(req): Json<RecordEventsRequest>,
) -> Result<StatusCode, ApiError> {
    state.db.record_events(Some(user.id), &req.events).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chaos_domain::{EventItem, ViewEvent, ViewEventItem};
    use chrono::Utc;

    use crate::config::Config;
    use crate::db::Db;

    async fn state_with_user() -> (AppState, chaos_domain::User) {
        let db = Db::in_memory().await.unwrap();
        let user = db.create_user("tibo", "Tibo", "x").await.unwrap();
        let state = AppState::new(Config::default(), db).unwrap();
        (state, user)
    }

    #[tokio::test]
    async fn views_map_unknown_source_is_404() {
        let (state, user) = state_with_user().await;
        let err = views_map(AuthUser(user), State(state), Path("nope".into()))
            .await
            .expect_err("unknown source must be rejected");
        assert!(matches!(err, ApiError::NotFound));
    }

    #[tokio::test]
    async fn record_views_then_map_round_trips() {
        let (state, user) = state_with_user().await;
        let req = RecordViewsRequest {
            events: vec![ViewEventItem {
                source: Source::HackerNews,
                post_id: "1".into(),
                event: ViewEvent::OpenedArticle,
                at: Utc::now(),
            }],
        };
        let code = record_views(AuthUser(user.clone()), State(state.clone()), Json(req))
            .await
            .unwrap();
        assert_eq!(code, StatusCode::NO_CONTENT);

        let map = views_map(AuthUser(user), State(state), Path("hackernews".into()))
            .await
            .unwrap()
            .0;
        let f = map.get("1").copied().unwrap();
        assert!(f.seen && f.article && !f.comments);
    }

    #[tokio::test]
    async fn record_events_returns_no_content() {
        let (state, user) = state_with_user().await;
        let req = RecordEventsRequest {
            events: vec![EventItem {
                kind: "app_open".into(),
                detail: None,
                at: Utc::now(),
            }],
        };
        let code = record_events(AuthUser(user), State(state.clone()), Json(req))
            .await
            .unwrap();
        assert_eq!(code, StatusCode::NO_CONTENT);
        assert_eq!(state.db.count_events("app_open").await.unwrap(), 1);
    }
}
