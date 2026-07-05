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
    /// Missing or invalid session (or bad login credentials).
    Unauthorized,
    /// Client mistakes: bad references, empty names, cycles…
    Unprocessable(String),
    /// An upstream service (widget provider) failed or timed out.
    BadGateway(String),
    Internal(String),
}

impl From<crate::widgets::WidgetError> for ApiError {
    fn from(err: crate::widgets::WidgetError) -> Self {
        use crate::widgets::WidgetError;
        match err {
            WidgetError::UnknownWidget => ApiError::NotFound,
            WidgetError::Rejected(msg) => ApiError::Unprocessable(msg),
            WidgetError::Upstream(reason) => ApiError::BadGateway(reason),
        }
    }
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
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_string()),
            ApiError::Unprocessable(msg) => (StatusCode::UNPROCESSABLE_ENTITY, msg),
            ApiError::BadGateway(msg) => (StatusCode::BAD_GATEWAY, msg),
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        (status, Json(ApiErrorBody { message })).into_response()
    }
}
