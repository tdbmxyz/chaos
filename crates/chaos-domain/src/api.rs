//! Request/response envelopes of the HTTP API (`/api/v1`).

use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use crate::links::{Link, Tag};

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

// ---- links ----

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateLinkRequest {
    pub url: Url,
    /// Derived from the URL when absent (metadata fetch comes later).
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub collection_id: Option<Uuid>,
    /// Tag names; unknown ones are created on the fly.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Full-replacement update (PUT semantics): every field is written as sent.
/// Simpler and less error-prone than sparse PATCH merging.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateLinkRequest {
    pub url: Url,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub collection_id: Option<Uuid>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Query parameters of `GET /api/v1/links`. Also used by chaos-client to
/// build the query string, so server and client cannot drift.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LinkQuery {
    pub collection_id: Option<Uuid>,
    /// Filter by tag name (exact, case-insensitive).
    pub tag: Option<String>,
    /// Substring search over title, description and url.
    pub q: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkPage {
    pub items: Vec<Link>,
    /// Total matching the filter, ignoring limit/offset.
    pub total: u64,
}

// ---- widgets ----

/// Body of `POST /api/v1/widgets/{id}/systemd`. The unit must be one of the
/// units configured on that widget instance (and marked controllable).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemdActionRequest {
    pub unit: String,
    pub action: SystemdAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemdAction {
    Start,
    Stop,
    Restart,
}

impl SystemdAction {
    /// The systemctl verb.
    pub fn verb(self) -> &'static str {
        match self {
            SystemdAction::Start => "start",
            SystemdAction::Stop => "stop",
            SystemdAction::Restart => "restart",
        }
    }
}

// ---- collections ----

/// Shared by create and update (update = full replacement).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub parent_id: Option<Uuid>,
}

// ---- tags ----

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagWithCount {
    #[serde(flatten)]
    pub tag: Tag,
    pub link_count: u64,
}
