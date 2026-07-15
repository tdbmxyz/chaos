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
