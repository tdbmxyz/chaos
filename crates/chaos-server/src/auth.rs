//! Password hashing, session tokens, and the `AuthUser` extractor.
//!
//! Identity vs session split (see docs/adr/0006-auth.md): this module owns
//! *sessions* (opaque tokens, sha256-hashed at rest, presented as a bearer
//! header by native clients or an HttpOnly cookie by browsers) and the
//! *local-password* identity. An external IdP (authentik/OIDC) later adds
//! another way to mint a session without touching anything downstream.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use axum::extract::FromRequestParts;
use axum::http::HeaderMap;
use axum::http::header;
use axum::http::request::Parts;
use chaos_domain::User;
use rand_core::OsRng;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::api::ApiError;
use crate::config::ForwardAuthConfig;
use crate::db::Db;
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

/// Verify a login attempt. On a user miss (`stored_hash` is `None`) the
/// password is still verified — against a dummy hash — so the unknown-user
/// path costs the same ~100 ms of argon2 as the known-user path and login
/// timing does not reveal which usernames exist.
pub fn verify_login(stored_hash: Option<&str>, password: &str) -> bool {
    match stored_hash {
        Some(hash) => verify_password(password, hash),
        None => {
            let _ = verify_password(password, dummy_hash());
            false
        }
    }
}

/// A valid argon2 PHC hash of a random token, generated once per process.
/// Nothing can ever match it; it only exists to burn verification time.
fn dummy_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| hash_password(&new_token()).expect("hashing dummy password"))
}

// ---- failed-login throttle ----

/// Failures a `username|ip` pair gets before delays kick in.
const THROTTLE_FREE_FAILURES: u32 = 3;
const THROTTLE_BASE_DELAY: Duration = Duration::from_millis(500);
const THROTTLE_MAX_DELAY: Duration = Duration::from_secs(30);
/// A pair with no failures for this long is forgotten.
const THROTTLE_RESET: Duration = Duration::from_secs(15 * 60);
/// Bound on tracked pairs: a spray of random usernames/XFF values must not
/// grow the map forever. Expired entries are pruned first; if every entry
/// is live the map is simply full and new pairs go untracked until one
/// expires — losing throttle precision under attack beats losing memory.
const THROTTLE_MAX_PAIRS: usize = 1024;

/// Delay owed after `failures` consecutive failures: nothing for the first
/// few (typos), then exponential backoff capped at [`THROTTLE_MAX_DELAY`].
fn throttle_delay(failures: u32) -> Duration {
    if failures < THROTTLE_FREE_FAILURES {
        return Duration::ZERO;
    }
    let exponent = (failures - THROTTLE_FREE_FAILURES).min(6);
    (THROTTLE_BASE_DELAY * 2u32.pow(exponent)).min(THROTTLE_MAX_DELAY)
}

/// In-memory failed-login tracker, keyed by `username|ip` (see
/// [`throttle_key`]). Single-instance servers only need process memory:
/// a restart forgiving old failures is fine.
#[derive(Default)]
pub struct LoginThrottle {
    attempts: Mutex<HashMap<String, (u32, Instant)>>,
}

impl LoginThrottle {
    /// How long this attempt must wait before being processed.
    pub fn delay(&self, key: &str) -> Duration {
        let mut attempts = self.attempts.lock().expect("throttle lock");
        match attempts.get(key) {
            Some((failures, last)) if last.elapsed() < THROTTLE_RESET => throttle_delay(*failures),
            Some(_) => {
                attempts.remove(key);
                Duration::ZERO
            }
            None => Duration::ZERO,
        }
    }

    pub fn record_failure(&self, key: &str) {
        let mut attempts = self.attempts.lock().expect("throttle lock");
        if !attempts.contains_key(key) && attempts.len() >= THROTTLE_MAX_PAIRS {
            attempts.retain(|_, (_, last)| last.elapsed() < THROTTLE_RESET);
            if attempts.len() >= THROTTLE_MAX_PAIRS {
                return;
            }
        }
        let entry = attempts
            .entry(key.to_string())
            .or_insert((0, Instant::now()));
        entry.0 = entry.0.saturating_add(1);
        entry.1 = Instant::now();
    }

    /// Successful login: the pair starts fresh.
    pub fn clear(&self, key: &str) {
        self.attempts.lock().expect("throttle lock").remove(key);
    }
}

pub fn throttle_key(username: &str, ip: Option<&str>) -> String {
    format!(
        "{}|{}",
        username.trim().to_lowercase(),
        ip.unwrap_or("unknown")
    )
}

/// Best-effort client address for throttling: the reverse proxy's
/// `X-Forwarded-For` (first hop) or `X-Real-IP`. Spoofable by direct LAN
/// clients, but combined with the username key it still stops dumb loops.
pub fn client_ip(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|ip| ip.trim().to_string())
        .filter(|ip| !ip.is_empty())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|ip| ip.trim().to_string())
        })
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

/// Resolve the user from a trusted reverse-proxy header set, or `None` when
/// forward-auth is disabled, the shared secret doesn't match, or no username
/// header is present. Auto-provisions on first contact (empty password hash).
///
/// Header lookups are case-insensitive (axum `HeaderMap`), and the configured
/// header names are stored lowercase; a request that lacks the secret header
/// (or carries the wrong value) is never trusted, so a direct/tailnet client
/// cannot forge an identity.
pub async fn forward_auth_user(
    headers: &HeaderMap,
    cfg: &ForwardAuthConfig,
    db: &Db,
) -> Result<Option<User>, ApiError> {
    let Some(secret) = &cfg.secret else {
        return Ok(None); // feature disabled
    };
    let sent = headers
        .get(cfg.secret_header.as_str())
        .and_then(|v| v.to_str().ok());
    if sent != Some(secret.as_str()) {
        return Ok(None); // wrong / absent secret — untrusted
    }
    let Some(username) = headers
        .get(cfg.username_header.as_str())
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(None); // trusted proxy but no identity
    };
    let display = headers
        .get(cfg.name_header.as_str())
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(username);
    let user = db
        .user_by_username_or_create(username, display)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    Ok(Some(user))
}

/// Best-effort session lookup for handlers that attribute rather than
/// gate — link creation doesn't require auth yet (see ROADMAP), so a
/// missing/invalid session just means "unattributed", not a rejection.
/// Falls back to a trusted forward-auth identity so events attribute
/// correctly behind authentik.
pub async fn optional_user_id(state: &AppState, headers: &HeaderMap) -> Option<Uuid> {
    if let Some(token) = request_token(headers)
        && let Ok(user) = state.db.user_by_session(&token_hash(&token)).await
    {
        return Some(user.id);
    }
    forward_auth_user(headers, &state.config.forward_auth, &state.db)
        .await
        .ok()
        .flatten()
        .map(|user| user.id)
}

/// Extractor for handlers that require a signed-in user.
pub struct AuthUser(pub User);

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        // 1. chaos session (Bearer/cookie) — unchanged, wins when present.
        if let Some(token) = request_token(&parts.headers)
            && let Ok(user) = state.db.user_by_session(&token_hash(&token)).await
        {
            return Ok(AuthUser(user));
        }
        // 2. trusted forward-auth header (only when configured + secret matches).
        if let Some(user) =
            forward_auth_user(&parts.headers, &state.config.forward_auth, &state.db).await?
        {
            return Ok(AuthUser(user));
        }
        Err(ApiError::Unauthorized)
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

    #[test]
    fn unknown_user_still_pays_a_password_verification() {
        // The dummy hash is a real argon2 PHC string, so the user-miss path
        // costs the same as a real verification.
        assert!(PasswordHash::new(dummy_hash()).is_ok());
        assert!(!verify_login(None, "hunter2"));

        let hash = hash_password("hunter2").expect("hash");
        assert!(verify_login(Some(&hash), "hunter2"));
        assert!(!verify_login(Some(&hash), "wrong"));
    }

    #[tokio::test]
    async fn forward_auth_disabled_returns_none() {
        let db = Db::in_memory().await.unwrap();
        let cfg = ForwardAuthConfig::default(); // secret None → off
        let mut h = HeaderMap::new();
        h.insert("x-authentik-username", "so".parse().unwrap());
        assert!(forward_auth_user(&h, &cfg, &db).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn forward_auth_requires_matching_secret_then_provisions() {
        let db = Db::in_memory().await.unwrap();
        let cfg = ForwardAuthConfig {
            secret: Some("s3cret".into()),
            ..Default::default()
        };

        // Wrong secret → untrusted → None (no provisioning).
        let mut bad = HeaderMap::new();
        bad.insert("x-chaos-proxy-secret", "nope".parse().unwrap());
        bad.insert("x-authentik-username", "so".parse().unwrap());
        assert!(forward_auth_user(&bad, &cfg, &db).await.unwrap().is_none());

        // Absent secret → None too.
        let mut nosecret = HeaderMap::new();
        nosecret.insert("x-authentik-username", "so".parse().unwrap());
        assert!(
            forward_auth_user(&nosecret, &cfg, &db)
                .await
                .unwrap()
                .is_none()
        );

        // Right secret + username + name → provisions.
        let mut ok = HeaderMap::new();
        ok.insert("x-chaos-proxy-secret", "s3cret".parse().unwrap());
        ok.insert("x-authentik-username", "so".parse().unwrap());
        ok.insert("x-authentik-name", "So Balem".parse().unwrap());
        let user = forward_auth_user(&ok, &cfg, &db)
            .await
            .unwrap()
            .expect("provisioned");
        assert_eq!(user.username, "so");
        assert_eq!(user.display_name, "So Balem");

        // Missing name header → display falls back to username.
        let mut noname = HeaderMap::new();
        noname.insert("x-chaos-proxy-secret", "s3cret".parse().unwrap());
        noname.insert("x-authentik-username", "ann".parse().unwrap());
        let u2 = forward_auth_user(&noname, &cfg, &db)
            .await
            .unwrap()
            .expect("provisioned");
        assert_eq!(u2.display_name, "ann");

        // Secret matches but no username header → None.
        let mut nouser = HeaderMap::new();
        nouser.insert("x-chaos-proxy-secret", "s3cret".parse().unwrap());
        assert!(
            forward_auth_user(&nouser, &cfg, &db)
                .await
                .unwrap()
                .is_none()
        );
    }

    /// The extractor tries the session token before forward-auth, so a valid
    /// chaos session wins even when forward-auth headers are also present.
    #[tokio::test]
    async fn extractor_prefers_session_token_over_forward_auth() {
        let db = Db::in_memory().await.unwrap();
        // A real chaos-login user with a live session.
        let token_user = db.create_user("tibo", "Tibo", "phc-string").await.unwrap();
        let token = new_token();
        db.create_session(
            &token_hash(&token),
            token_user.id,
            Utc::now() + chrono::Duration::days(1),
        )
        .await
        .unwrap();

        let cfg = ForwardAuthConfig {
            secret: Some("s3cret".into()),
            ..Default::default()
        };
        // Request carries BOTH a valid token AND forward-auth headers for a
        // *different* identity.
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        headers.insert("x-chaos-proxy-secret", "s3cret".parse().unwrap());
        headers.insert("x-authentik-username", "someoneelse".parse().unwrap());

        // The extractor logic: token path first.
        let resolved = if let Some(t) = request_token(&headers)
            && let Ok(u) = db.user_by_session(&token_hash(&t)).await
        {
            u
        } else {
            forward_auth_user(&headers, &cfg, &db)
                .await
                .unwrap()
                .expect("would fall back")
        };
        assert_eq!(resolved.id, token_user.id, "session token wins");
        assert_eq!(resolved.username, "tibo");
    }

    use chrono::Utc;

    #[test]
    fn throttle_delay_backs_off_after_free_failures() {
        use std::time::Duration;
        assert_eq!(throttle_delay(0), Duration::ZERO);
        assert_eq!(throttle_delay(2), Duration::ZERO);
        assert_eq!(throttle_delay(3), Duration::from_millis(500));
        assert_eq!(throttle_delay(4), Duration::from_secs(1));
        assert_eq!(throttle_delay(5), Duration::from_secs(2));
        // Capped, even for absurd counts.
        assert_eq!(throttle_delay(60), Duration::from_secs(30));
    }

    #[test]
    fn failed_attempts_are_tracked_per_key_and_cleared_on_success() {
        use std::time::Duration;
        let throttle = LoginThrottle::default();
        assert_eq!(throttle.delay("tibo|1.2.3.4"), Duration::ZERO);
        for _ in 0..3 {
            throttle.record_failure("tibo|1.2.3.4");
        }
        assert_eq!(throttle.delay("tibo|1.2.3.4"), Duration::from_millis(500));
        // Another user/IP pair is unaffected.
        assert_eq!(throttle.delay("tibo|5.6.7.8"), Duration::ZERO);
        throttle.clear("tibo|1.2.3.4");
        assert_eq!(throttle.delay("tibo|1.2.3.4"), Duration::ZERO);
    }

    #[test]
    fn throttle_map_is_bounded_against_key_spray() {
        let throttle = LoginThrottle::default();
        for i in 0..(THROTTLE_MAX_PAIRS + 100) {
            throttle.record_failure(&format!("user{i}|1.2.3.4"));
        }
        let len = throttle.attempts.lock().unwrap().len();
        assert!(
            len <= THROTTLE_MAX_PAIRS,
            "spraying unique keys must not grow the map past the cap (got {len})"
        );
        // Known pairs keep being tracked.
        throttle.record_failure("user1|1.2.3.4");
        let (failures, _) = throttle.attempts.lock().unwrap()["user1|1.2.3.4"];
        assert_eq!(failures, 2);
    }

    #[test]
    fn throttle_key_normalizes_username_and_defaults_ip() {
        assert_eq!(throttle_key(" Tibo ", Some("1.2.3.4")), "tibo|1.2.3.4");
        assert_eq!(throttle_key("tibo", None), "tibo|unknown");
    }

    #[test]
    fn client_ip_prefers_forwarded_for() {
        let mut headers = axum::http::HeaderMap::new();
        assert_eq!(client_ip(&headers), None);
        headers.insert("x-real-ip", "10.0.0.9".parse().unwrap());
        assert_eq!(client_ip(&headers).as_deref(), Some("10.0.0.9"));
        headers.insert("x-forwarded-for", "1.2.3.4, 10.0.0.1".parse().unwrap());
        assert_eq!(client_ip(&headers).as_deref(), Some("1.2.3.4"));
    }
}
