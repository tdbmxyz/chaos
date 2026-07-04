use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chaos_domain::{Collection, CollectionRequest};
use uuid::Uuid;

use super::ApiError;
use crate::state::AppState;

pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<Collection>>, ApiError> {
    Ok(Json(state.db.list_collections().await?))
}

pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CollectionRequest>,
) -> Result<(StatusCode, Json<Collection>), ApiError> {
    let collection = state.db.create_collection(&req).await?;
    Ok((StatusCode::CREATED, Json(collection)))
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<CollectionRequest>,
) -> Result<Json<Collection>, ApiError> {
    Ok(Json(state.db.update_collection(id, &req).await?))
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    state.db.delete_collection(id).await?;
    Ok(StatusCode::NO_CONTENT)
}
