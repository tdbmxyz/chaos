use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chaos_domain::ApiErrorBody;

use crate::db::DbError;

/// API-level error: every handler returns `Result<_, ApiError>` and the
/// uniform `ApiErrorBody` envelope goes over the wire.
#[derive(Debug)]
pub enum ApiError {
    NotFound,
    /// Client mistakes: bad references, empty names, cycles…
    Unprocessable(String),
    Internal(String),
}

impl From<DbError> for ApiError {
    fn from(err: DbError) -> Self {
        match err {
            DbError::NotFound => ApiError::NotFound,
            DbError::Constraint(msg) => ApiError::Unprocessable(msg),
            DbError::Corrupt(_) | DbError::Sqlx(_) | DbError::Migrate(_) => {
                // Log the detail, keep the wire message generic.
                tracing::error!(error = %err, "database error");
                ApiError::Internal("internal error".into())
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            ApiError::Unprocessable(msg) => (StatusCode::UNPROCESSABLE_ENTITY, msg),
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        (status, Json(ApiErrorBody { message })).into_response()
    }
}
