//! Request/response envelopes of the HTTP API (`/api/v1`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    /// Server crate version, useful to detect client/server drift.
    pub version: String,
}

/// Uniform error body returned by the API for non-2xx responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub message: String,
}
