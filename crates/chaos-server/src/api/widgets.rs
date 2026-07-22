//! Dashboard layout and widget payload endpoints.

use axum::Json;
use axum::extract::{Path, State};
use chaos_domain::{DashboardLayout, PostThread, SystemdActionRequest, WidgetData};

use crate::api::ApiError;
use crate::state::AppState;

pub async fn dashboard(State(state): State<AppState>) -> Json<DashboardLayout> {
    Json(state.widgets.layout.clone())
}

pub async fn widget_data(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<WidgetData>, ApiError> {
    state.widgets.data(&id).await.map(Json).map_err(Into::into)
}

/// Standalone posts feed for the `/news` page, keyed by source
/// (`hackernews`/`lobsters`). Unknown sources 404.
pub async fn posts_list(
    State(state): State<AppState>,
    Path(source): Path<String>,
) -> Result<Json<WidgetData>, ApiError> {
    let source = chaos_domain::Source::from_str(&source).ok_or(ApiError::NotFound)?;
    let data = state.widgets.posts_list(source).await?;
    // Record post-ingestion timestamps (first_seen_at) for analytics.
    // Best-effort: never fail the response over this.
    if let WidgetData::Posts(posts) = &data {
        let items: Vec<(String, String, String)> = posts
            .last_24h
            .iter()
            .chain(&posts.last_48h)
            .chain(&posts.last_week)
            .filter_map(|i| {
                i.id.clone()
                    .map(|id| (source.as_str().to_string(), id, i.title.clone()))
            })
            .collect();
        let _ = state.db.upsert_posts(&items, chrono::Utc::now()).await;
    }
    Ok(Json(data))
}

/// Comment thread for one post (`source` + provider id), served by the
/// reader. Unknown sources 404.
pub async fn post_thread(
    State(state): State<AppState>,
    Path((source, id)): Path<(String, String)>,
) -> Result<Json<PostThread>, ApiError> {
    let source = chaos_domain::Source::from_str(&source).ok_or(ApiError::NotFound)?;
    state
        .widgets
        .post_thread(source, &id)
        .await
        .map(Json)
        .map_err(Into::into)
}

pub async fn widget_systemd(
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::Config;
    use crate::db::Db;

    #[tokio::test]
    async fn posts_list_unknown_source_is_404() {
        let db = Db::in_memory().await.unwrap();
        let state = AppState::new(Config::default(), db).unwrap();
        let err = posts_list(State(state), Path("nope".into()))
            .await
            .expect_err("unknown source must be rejected");
        assert!(matches!(err, ApiError::NotFound));
    }

    #[tokio::test]
    async fn thread_unknown_source_is_404() {
        let db = Db::in_memory().await.unwrap();
        let state = AppState::new(Config::default(), db).unwrap();
        let err = post_thread(State(state), Path(("nope".into(), "1".into())))
            .await
            .expect_err("unknown source must be rejected");
        assert!(matches!(err, ApiError::NotFound));
    }
}
