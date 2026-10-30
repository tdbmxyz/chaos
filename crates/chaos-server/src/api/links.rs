use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chaos_domain::{
    ArchiveState, CreateLinkRequest, Link, LinkPage, LinkQuery, TagWithCount, UpdateLinkRequest,
};
use uuid::Uuid;

use super::ApiError;
use crate::auth::AuthUser;
use crate::state::AppState;
use crate::{archiver, metadata};

fn is_blank(value: &Option<String>) -> bool {
    value.as_deref().map(str::trim).is_none_or(str::is_empty)
}

pub async fn list(
    AuthUser(_user): AuthUser,
    State(state): State<AppState>,
    Query(query): Query<LinkQuery>,
) -> Result<Json<LinkPage>, ApiError> {
    Ok(Json(state.db.list_links(&query).await?))
}

pub async fn get_one(
    AuthUser(_user): AuthUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Link>, ApiError> {
    Ok(Json(state.db.get_link(id).await?))
}

pub async fn create(
    AuthUser(_user): AuthUser,
    State(state): State<AppState>,
    headers: HeaderMap,
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

    let created_by = crate::auth::optional_user_id(&state, &headers).await;
    let auto_archive = state.config.archive.enabled && state.config.archive.auto;
    let link = state.db.create_link(&req, auto_archive, created_by).await?;
    if auto_archive {
        state.archive_notify.notify_one();
    }
    Ok((StatusCode::CREATED, Json(link)))
}

pub async fn update(
    AuthUser(_user): AuthUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateLinkRequest>,
) -> Result<Json<Link>, ApiError> {
    Ok(Json(state.db.update_link(id, &req).await?))
}

pub async fn delete(
    AuthUser(_user): AuthUser,
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
    AuthUser(_user): AuthUser,
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
    AuthUser(_user): AuthUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let link = state.db.get_link(id).await?;
    if !matches!(link.archive, ArchiveState::Archived { .. }) {
        return Err(ApiError::NotFound);
    }
    let bytes = tokio::fs::read(archiver::snapshot_path(&state, id))
        .await
        .map_err(|_| ApiError::NotFound)?;

    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static(archiver::snapshot_content_type(&bytes)),
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
        bytes,
    )
        .into_response())
}

pub async fn tags(
    AuthUser(_user): AuthUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<TagWithCount>>, ApiError> {
    Ok(Json(state.db.list_tags().await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::db::{ArchiveOutcome, Db};

    #[tokio::test]
    async fn archives_are_served_with_their_content_type_and_original_bytes() {
        let dir = std::env::temp_dir().join(format!("chaos-serve-archive-{}", Uuid::now_v7()));
        let mut config = Config::default();
        config.archive.dir = dir.clone();
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let db = Db::in_memory().await.unwrap();
        let user = db.create_user("test", "Test", "").await.unwrap();
        let state = AppState::new(config, db).unwrap();

        for (bytes, content_type) in [
            (
                b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".as_slice(),
                "application/pdf",
            ),
            (b"<html>hello</html>".as_slice(), "text/html; charset=utf-8"),
        ] {
            // No extension: detection must depend on the stored bytes.
            let link = state
                .db
                .create_link(
                    &CreateLinkRequest {
                        url: format!("https://example.com/{}", Uuid::now_v7())
                            .parse()
                            .unwrap(),
                        title: None,
                        description: None,
                        collection_id: None,
                        tags: vec![],
                    },
                    true,
                    None,
                )
                .await
                .unwrap();
            tokio::fs::write(archiver::snapshot_path(&state, link.id), bytes)
                .await
                .unwrap();
            state
                .db
                .finish_archive(
                    link.id,
                    ArchiveOutcome::Success {
                        size_bytes: bytes.len() as u64,
                        text: String::new(),
                    },
                )
                .await
                .unwrap();

            let response =
                serve_archive(AuthUser(user.clone()), State(state.clone()), Path(link.id))
                    .await
                    .unwrap();
            assert_eq!(response.headers()[header::CONTENT_TYPE], content_type);
            assert!(
                response.headers()[header::CONTENT_SECURITY_POLICY]
                    .to_str()
                    .unwrap()
                    .contains("sandbox")
            );
            let body = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap();
            assert_eq!(body.as_ref(), bytes);
        }
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
