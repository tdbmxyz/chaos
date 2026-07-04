use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use chaos_domain::{CreateLinkRequest, Link, LinkPage, LinkQuery, TagWithCount, UpdateLinkRequest};
use uuid::Uuid;

use super::ApiError;
use crate::metadata;
use crate::state::AppState;

fn is_blank(value: &Option<String>) -> bool {
    value.as_deref().map(str::trim).is_none_or(str::is_empty)
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<LinkQuery>,
) -> Result<Json<LinkPage>, ApiError> {
    Ok(Json(state.db.list_links(&query).await?))
}

pub async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Link>, ApiError> {
    Ok(Json(state.db.get_link(id).await?))
}

pub async fn create(
    State(state): State<AppState>,
    Json(mut req): Json<CreateLinkRequest>,
) -> Result<(StatusCode, Json<Link>), ApiError> {
    // Fill missing title/description from the page itself. Best-effort:
    // on fetch failure the db layer still falls back to the URL host.
    if is_blank(&req.title) || is_blank(&req.description) {
        let meta = metadata::fetch(&state.fetcher, &req.url).await;
        if is_blank(&req.title) {
            req.title = meta.title;
        }
        if is_blank(&req.description) {
            req.description = meta.description;
        }
    }

    let link = state.db.create_link(&req).await?;
    Ok((StatusCode::CREATED, Json(link)))
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateLinkRequest>,
) -> Result<Json<Link>, ApiError> {
    Ok(Json(state.db.update_link(id, &req).await?))
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    state.db.delete_link(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn tags(State(state): State<AppState>) -> Result<Json<Vec<TagWithCount>>, ApiError> {
    Ok(Json(state.db.list_tags().await?))
}
