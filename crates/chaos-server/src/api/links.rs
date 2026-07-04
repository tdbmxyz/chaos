use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chaos_domain::{
    ArchiveState, CreateLinkRequest, Link, LinkPage, LinkQuery, TagWithCount, UpdateLinkRequest,
};
use uuid::Uuid;

use super::ApiError;
use crate::state::AppState;
use crate::{archiver, metadata};

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

    let auto_archive = state.config.archive.enabled && state.config.archive.auto;
    let link = state.db.create_link(&req, auto_archive).await?;
    if auto_archive {
        state.archive_notify.notify_one();
    }
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
    // Snapshot file may or may not exist; either way the link is gone.
    let _ = tokio::fs::remove_file(archiver::snapshot_path(&state, id)).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Queue (or re-queue) a snapshot of the page.
pub async fn rearchive(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<Link>), ApiError> {
    if !state.config.archive.enabled {
        return Err(ApiError::Unprocessable("archiving is disabled".into()));
    }
    let link = state.db.set_archive_pending(id).await?;
    state.archive_notify.notify_one();
    Ok((StatusCode::ACCEPTED, Json(link)))
}

/// Serve the stored single-file snapshot.
pub async fn serve_archive(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let link = state.db.get_link(id).await?;
    if !matches!(link.archive, ArchiveState::Archived { .. }) {
        return Err(ApiError::NotFound);
    }
    let html = tokio::fs::read(archiver::snapshot_path(&state, id))
        .await
        .map_err(|_| ApiError::NotFound)?;

    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            // Belt and braces on top of monolith's -j/-I: no scripts, no
            // outbound requests from archived pages.
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(
                    "sandbox allow-same-origin; default-src data: 'unsafe-inline'",
                ),
            ),
        ],
        html,
    )
        .into_response())
}

pub async fn tags(State(state): State<AppState>) -> Result<Json<Vec<TagWithCount>>, ApiError> {
    Ok(Json(state.db.list_tags().await?))
}
