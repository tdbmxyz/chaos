//! Users and sessions.
//!
//! Authentication is deliberately split in two layers: *identity* (how a
//! user proves who they are — local password today, authentik/OIDC later)
//! and *session* (an opaque bearer token / cookie both web and native
//! clients present on every request). Adding an external IdP later only
//! adds a new way to obtain a session; everything downstream is unchanged.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    pub display_name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginResponse {
    /// Session token. Browsers also get it as an HttpOnly cookie; native
    /// clients keep it and send `Authorization: Bearer <token>`.
    pub token: String,
    pub user: User,
}
