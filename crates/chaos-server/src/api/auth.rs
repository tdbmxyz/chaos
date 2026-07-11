//! `/api/v1/auth/*`: login, logout, whoami.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use chaos_domain::{LoginRequest, LoginResponse, User};
use chrono::{Duration, Utc};

use crate::api::ApiError;
use crate::auth::{
    AuthUser, SESSION_COOKIE, SESSION_DAYS, client_ip, new_token, request_token, throttle_key,
    token_hash, verify_login,
};
use crate::state::AppState;

pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    // Repeated failures for the same username+IP earn an increasing delay.
    let key = throttle_key(&req.username, client_ip(&headers).as_deref());
    let delay = state.login_throttle.delay(&key);
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }

    // Same rejection — and the same argon2 cost — for unknown user and
    // wrong password (see verify_login).
    let found = state.db.user_with_password(&req.username).await.ok();
    if !verify_login(found.as_ref().map(|(_, hash)| hash.as_str()), &req.password) {
        state.login_throttle.record_failure(&key);
        return Err(ApiError::Unauthorized);
    }
    let (user, _) = found.expect("verify_login returns false when the user is missing");
    state.login_throttle.clear(&key);

    let token = new_token();
    state
        .db
        .create_session(
            &token_hash(&token),
            user.id,
            Utc::now() + Duration::days(SESSION_DAYS),
        )
        .await?;
    tracing::info!(username = user.username, "login");

    Ok((
        session_cookie_headers(
            &token,
            SESSION_DAYS * 24 * 60 * 60,
            state.config.secure_cookies,
        ),
        Json(LoginResponse { token, user }),
    )
        .into_response())
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(token) = request_token(&headers) {
        let _ = state.db.delete_session(&token_hash(&token)).await;
    }
    // Expire the cookie either way.
    (
        session_cookie_headers("", 0, state.config.secure_cookies),
        Json(serde_json::json!({})),
    )
        .into_response()
}

pub async fn me(AuthUser(user): AuthUser) -> Json<User> {
    Json(user)
}

fn session_cookie_headers(token: &str, max_age_secs: i64, secure: bool) -> HeaderMap {
    let mut cookie =
        format!("{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_secs}");
    if secure {
        cookie.push_str("; Secure");
    }
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        headers.insert(header::SET_COOKIE, value);
    }
    headers
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header;

    fn cookie(headers: &HeaderMap) -> &str {
        headers
            .get(header::SET_COOKIE)
            .expect("set-cookie present")
            .to_str()
            .expect("ascii cookie")
    }

    #[test]
    fn session_cookie_secure_flag_follows_config() {
        let plain = session_cookie_headers("tok", 60, false);
        assert!(!cookie(&plain).contains("Secure"));
        assert!(cookie(&plain).contains("HttpOnly"));

        let secure = session_cookie_headers("tok", 60, true);
        assert!(cookie(&secure).ends_with("; Secure"));
    }
}
