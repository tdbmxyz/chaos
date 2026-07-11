//! Dashboard layout, widget payload and weather endpoints.

use axum::Json;
use axum::extract::{Path, Query, State};
use chaos_domain::{DashboardLayout, SystemdActionRequest, WidgetData};

use crate::api::ApiError;
use crate::state::AppState;

pub async fn dashboard(State(state): State<AppState>) -> Json<DashboardLayout> {
    Json(state.widgets.layout.clone())
}

#[derive(serde::Deserialize)]
pub(crate) struct WidgetQuery {
    /// Device preference: weather widgets fetch this location instead of
    /// the configured one.
    location: Option<String>,
}

pub async fn widget_data(
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
pub async fn weather(
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
