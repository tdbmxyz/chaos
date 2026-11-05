//! Users and sessions.
//!
//! Local password login issues an opaque session token, used as a browser
//! cookie or a bearer header for explicit cross-origin API clients. Trusted
//! reverse-proxy authentication resolves the same user type without creating
//! a local session.

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
