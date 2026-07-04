//! Icon proxy with on-disk cache.
//!
//! The frontend references icons as `di:name` (dashboard-icons),
//! `si:name` (Simple Icons) or `sh:name` (selfh.st icons) — the same
//! conventions as glance. The server fetches them once and caches forever,
//! so the dashboard works without any client-side internet access.

use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};

use super::ApiError;
use crate::state::AppState;

/// Icons never change for a given name; a month of client caching is safe.
const CACHE_CONTROL: &str = "public, max-age=2592000, immutable";
const MAX_ICON_BYTES: usize = 1024 * 1024;

pub async fn icon(
    State(state): State<AppState>,
    Path(spec): Path<String>,
) -> Result<Response, ApiError> {
    let (kind, name) = spec.split_once(':').ok_or(ApiError::NotFound)?;
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(ApiError::NotFound);
    }

    let (upstream, content_type, ext) = match kind {
        "di" => (
            format!("https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons/png/{name}.png"),
            "image/png",
            "png",
        ),
        "sh" => (
            format!("https://cdn.jsdelivr.net/gh/selfhst/icons/png/{name}.png"),
            "image/png",
            "png",
        ),
        "si" => (
            format!("https://cdn.simpleicons.org/{name}"),
            "image/svg+xml",
            "svg",
        ),
        _ => return Err(ApiError::NotFound),
    };

    let cache_path = state
        .config
        .icon_cache_dir
        .join(format!("{kind}-{name}.{ext}"));
    let bytes = match tokio::fs::read(&cache_path).await {
        Ok(bytes) => bytes,
        Err(_) => {
            let bytes = fetch_icon(&state, &upstream).await?;
            if let Some(parent) = cache_path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            // Cache write failure only costs a refetch next time.
            let _ = tokio::fs::write(&cache_path, &bytes).await;
            bytes
        }
    };

    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static(CACHE_CONTROL),
            ),
        ],
        bytes,
    )
        .into_response())
}

async fn fetch_icon(state: &AppState, url: &str) -> Result<Vec<u8>, ApiError> {
    let resp = state
        .fetcher
        .get(url)
        .send()
        .await
        .map_err(|_| ApiError::NotFound)?;
    if !resp.status().is_success() {
        return Err(ApiError::NotFound);
    }
    let bytes = resp.bytes().await.map_err(|_| ApiError::NotFound)?;
    if bytes.len() > MAX_ICON_BYTES {
        return Err(ApiError::NotFound);
    }
    Ok(bytes.to_vec())
}
