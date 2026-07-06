//! Password hashing, session tokens, and the `AuthUser` extractor.
//!
//! Identity vs session split (see docs/adr/0006-auth.md): this module owns
//! *sessions* (opaque tokens, sha256-hashed at rest, presented as a bearer
//! header by native clients or an HttpOnly cookie by browsers) and the
//! *local-password* identity. An external IdP (authentik/OIDC) later adds
//! another way to mint a session without touching anything downstream.

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use axum::extract::FromRequestParts;
use axum::http::header;
use axum::http::request::Parts;
use chaos_domain::User;
use rand_core::OsRng;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::api::ApiError;
use crate::state::AppState;

pub const SESSION_COOKIE: &str = "chaos_session";
pub const SESSION_DAYS: i64 = 90;

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| anyhow::anyhow!("hashing password: {e}"))
}

pub fn verify_password(password: &str, phc: &str) -> bool {
    PasswordHash::new(phc)
        .map(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

/// Opaque session token: 244 bits of OS randomness, hex-encoded.
pub fn new_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// What is stored in the sessions table (the raw token never touches disk).
pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// The session token presented by this request, from `Authorization:
/// Bearer …` (native clients) or the session cookie (browsers).
pub fn request_token(headers: &axum::http::HeaderMap) -> Option<String> {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim().to_string());
    if bearer.is_some() {
        return bearer;
    }
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())?
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .map(|(_, value)| value.to_string())
}

/// Best-effort session lookup for handlers that attribute rather than
/// gate — link creation doesn't require auth yet (see ROADMAP), so a
/// missing/invalid session just means "unattributed", not a rejection.
pub async fn optional_user_id(state: &AppState, headers: &axum::http::HeaderMap) -> Option<Uuid> {
    let token = request_token(headers)?;
    state
        .db
        .user_by_session(&token_hash(&token))
        .await
        .ok()
        .map(|user| user.id)
}

/// Extractor for handlers that require a signed-in user.
pub struct AuthUser(pub User);

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let token = request_token(&parts.headers).ok_or(ApiError::Unauthorized)?;
        let user = state
            .db
            .user_by_session(&token_hash(&token))
            .await
            .map_err(|_| ApiError::Unauthorized)?;
        Ok(AuthUser(user))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let hash = hash_password("hunter2").expect("hash");
        assert!(verify_password("hunter2", &hash));
        assert!(!verify_password("hunter3", &hash));
        assert!(!verify_password("hunter2", "not-a-phc-string"));
    }

    #[test]
    fn request_token_prefers_bearer_over_cookie() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "a=b; chaos_session=cookie-tok".parse().unwrap(),
        );
        assert_eq!(request_token(&headers).as_deref(), Some("cookie-tok"));
        headers.insert(header::AUTHORIZATION, "Bearer bearer-tok".parse().unwrap());
        assert_eq!(request_token(&headers).as_deref(), Some("bearer-tok"));
    }
}
