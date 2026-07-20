//! Dashboard layout and widget payload endpoints.

use axum::Json;
use axum::extract::{Path, State};
use chaos_domain::{DashboardLayout, SystemdActionRequest, WidgetData};

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
    state
        .widgets
        .posts_list(source)
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
}
