//! OIDC sign-in for the shells: authorization code + PKCE, token refresh, and
//! durable token storage.
//!
//! All of it runs natively rather than in the webview. Two reasons: authentik
//! sends no `Access-Control-Allow-Origin` for `tauri://localhost` on a provider
//! whose redirect URI is a custom scheme, so a webview-side exchange is blocked
//! outright; and the refresh token — the long-lived credential — never touches
//! webview storage, which is neither durable across reinstalls nor private to
//! the app's own code.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Where authentik sends the browser back to. Registered as an intent-filter
/// on Android and a scheme handler on the desktop; must match the provider's
/// redirect URI exactly.
pub const REDIRECT_URI: &str = "xyz.tdbm.chaos://auth/callback";

/// Refresh this long before the access token actually expires, so a request
/// never races the expiry.
pub const REFRESH_MARGIN_SECS: i64 = 300;

/// The PKCE pair for one in-flight sign-in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pending {
    pub verifier: String,
    pub state: String,
    pub issuer: String,
    pub client_id: String,
}

/// `S256` code challenge: base64url(sha256(verifier)), no padding (RFC 7636).
pub fn code_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// A fresh high-entropy URL-safe string for a verifier or a state value.
pub fn random_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    // 32 bytes → 43 base64url characters, the RFC's minimum verifier length.
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The full authorization URL to open in the system browser.
pub fn authorize_url(authorization_endpoint: &str, pending: &Pending) -> String {
    let challenge = code_challenge(&pending.verifier);
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("response_type", "code")
        .append_pair("client_id", &pending.client_id)
        .append_pair("redirect_uri", REDIRECT_URI)
        // offline_access is what makes authentik issue a refresh token at all;
        // without it the session dies with the access token an hour later and
        // the provider's refresh-token validity is meaningless. The matching
        // scope mapping has to be assigned to the provider too.
        .append_pair("scope", "openid profile email offline_access")
        .append_pair("state", &pending.state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .finish();
    format!("{authorization_endpoint}?{query}")
}

/// Pull `code` and `state` out of a deep-link callback URL. `None` when this
/// isn't a callback we started (wrong scheme/path, or missing parameters).
pub fn parse_callback(url: &str) -> Option<(String, String)> {
    let parsed = url::Url::parse(url).ok()?;
    if parsed.scheme() != "xyz.tdbm.chaos" {
        return None;
    }
    // Custom-scheme URLs put "auth" in the host and "/callback" in the path.
    if parsed.host_str() != Some("auth") || parsed.path() != "/callback" {
        return None;
    }
    let mut code = None;
    let mut state = None;
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            _ => {}
        }
    }
    Some((code?, state?))
}

/// Whether a stored access token needs refreshing, given its expiry and now
/// (both unix seconds).
pub fn needs_refresh(expires_at: i64, now: i64) -> bool {
    expires_at - now <= REFRESH_MARGIN_SECS
}

use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;

/// Where tokens live: a JSON file in the app data directory, which survives
/// app updates (webview localStorage does not survive a reinstall, and is
/// readable by any script running in the webview).
const STORE_FILE: &str = "auth.json";
const KEY_REFRESH: &str = "refresh_token";
const KEY_ACCESS: &str = "access_token";
const KEY_EXPIRES: &str = "expires_at";
const KEY_PENDING: &str = "pending";
const KEY_ISSUER: &str = "issuer";
const KEY_CLIENT: &str = "client_id";

#[derive(Debug, Deserialize)]
struct Discovery {
    authorization_endpoint: String,
    token_endpoint: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Every network call here is on a path the user is actively waiting on, and a
/// stalled connection on mobile data must not hang sign-in forever.
fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .unwrap_or_default()
}

async fn discover(issuer: &str) -> Result<Discovery, String> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    http()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("cannot reach the identity provider: {e}"))?
        .json::<Discovery>()
        .await
        .map_err(|e| format!("unexpected discovery document: {e}"))
}

/// Persist a token response. A refresh response that omits `refresh_token`
/// (authentik does this when rotation is off) keeps the existing one.
fn store_tokens<R: Runtime>(app: &AppHandle<R>, tokens: &TokenResponse) -> Result<(), String> {
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    store.set(KEY_ACCESS, tokens.access_token.clone());
    store.set(KEY_EXPIRES, now() + tokens.expires_in.unwrap_or(3600));
    if let Some(refresh) = &tokens.refresh_token {
        store.set(KEY_REFRESH, refresh.clone());
    }
    store.save().map_err(|e| e.to_string())
}

/// Start a sign-in: remember the PKCE pair, return the URL to open.
#[tauri::command]
pub async fn auth_start<R: Runtime>(
    app: AppHandle<R>,
    issuer: String,
    client_id: String,
) -> Result<String, String> {
    let discovery = discover(&issuer).await?;
    let pending = Pending {
        verifier: random_token(),
        state: random_token(),
        issuer: issuer.clone(),
        client_id: client_id.clone(),
    };
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    store.set(
        KEY_PENDING,
        serde_json::to_value(&pending).map_err(|e| e.to_string())?,
    );
    store.set(KEY_ISSUER, issuer);
    store.set(KEY_CLIENT, client_id);
    store.save().map_err(|e| e.to_string())?;
    Ok(authorize_url(&discovery.authorization_endpoint, &pending))
}

/// Finish a sign-in from the deep-link callback.
pub async fn finish<R: Runtime>(app: &AppHandle<R>, code: &str, state: &str) -> Result<(), String> {
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    let pending: Pending = store
        .get(KEY_PENDING)
        .and_then(|v| serde_json::from_value(v).ok())
        .ok_or("no sign-in is in progress")?;
    // The state check is what stops a callback we didn't start from planting
    // someone else's session in this app.
    if pending.state != state {
        return Err("sign-in state did not match; ignoring this callback".into());
    }
    let discovery = discover(&pending.issuer).await?;
    let response = http()
        .post(&discovery.token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", &pending.client_id),
            ("code_verifier", &pending.verifier),
        ])
        .send()
        .await
        .map_err(|e| format!("token request failed: {e}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "identity provider rejected the code ({status}): {body}"
        ));
    }
    let tokens: TokenResponse = response
        .json()
        .await
        .map_err(|e| format!("unexpected token response: {e}"))?;
    store.delete(KEY_PENDING);
    store.save().map_err(|e| e.to_string())?;
    store_tokens(app, &tokens)
}

/// The current access token, refreshed when it is at or near expiry. `None`
/// means "not signed in" — the UI then shows the sign-in gate.
#[tauri::command]
pub async fn auth_token<R: Runtime>(app: AppHandle<R>) -> Result<Option<String>, String> {
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    // Reload first: on the desktop the callback may have been handled by
    // another instance, which wrote tokens to this same file.
    let _ = store.reload();
    let access = store
        .get(KEY_ACCESS)
        .and_then(|v| v.as_str().map(String::from));
    let expires_at = store.get(KEY_EXPIRES).and_then(|v| v.as_i64()).unwrap_or(0);
    if let Some(access) = access
        && !needs_refresh(expires_at, now())
    {
        return Ok(Some(access));
    }
    let (Some(refresh), Some(issuer), Some(client_id)) = (
        store
            .get(KEY_REFRESH)
            .and_then(|v| v.as_str().map(String::from)),
        store
            .get(KEY_ISSUER)
            .and_then(|v| v.as_str().map(String::from)),
        store
            .get(KEY_CLIENT)
            .and_then(|v| v.as_str().map(String::from)),
    ) else {
        return Ok(None);
    };
    let discovery = discover(&issuer).await?;
    let response = http()
        .post(&discovery.token_endpoint)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh),
            ("client_id", &client_id),
        ])
        .send()
        .await
        .map_err(|e| format!("refresh failed: {e}"))?;
    if !response.status().is_success() {
        // A refused refresh (revoked, expired, rotated away) is a real
        // sign-out; anything else leaves the stored session alone so a flaky
        // network doesn't log the user out.
        if response.status().as_u16() == 400 {
            sign_out_store(&store)?;
        }
        return Ok(None);
    }
    let tokens: TokenResponse = response
        .json()
        .await
        .map_err(|e| format!("unexpected refresh response: {e}"))?;
    let access = tokens.access_token.clone();
    store_tokens(&app, &tokens)?;
    Ok(Some(access))
}

fn sign_out_store<R: Runtime>(
    store: &std::sync::Arc<tauri_plugin_store::Store<R>>,
) -> Result<(), String> {
    store.delete(KEY_ACCESS);
    store.delete(KEY_REFRESH);
    store.delete(KEY_EXPIRES);
    store.delete(KEY_PENDING);
    store.save().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn auth_sign_out<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    sign_out_store(&store)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The worked example from RFC 7636 appendix B — if this passes, our
    /// challenge derivation is byte-for-byte what authentik expects.
    #[test]
    fn code_challenge_matches_the_rfc_test_vector() {
        assert_eq!(
            code_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn random_tokens_are_long_url_safe_and_unique() {
        let a = random_token();
        let b = random_token();
        assert_ne!(a, b);
        // RFC 7636 requires 43..=128 characters for a verifier.
        assert!((43..=128).contains(&a.len()), "length was {}", a.len());
        assert!(
            a.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "not url-safe: {a}"
        );
    }

    fn pending() -> Pending {
        Pending {
            verifier: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into(),
            state: "state-123".into(),
            issuer: "https://auth.example/application/o/chaos-app/".into(),
            client_id: "client-abc".into(),
        }
    }

    #[test]
    fn authorize_url_carries_pkce_and_the_redirect() {
        let url = authorize_url("https://auth.example/application/o/authorize/", &pending());
        assert!(url.starts_with("https://auth.example/application/o/authorize/?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=client-abc"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"));
        assert!(url.contains("state=state-123"));
        // The redirect URI must be percent-encoded, colons and slashes included.
        assert!(url.contains("redirect_uri=xyz.tdbm.chaos%3A%2F%2Fauth%2Fcallback"));
        assert!(
            url.contains("scope=openid+profile+email")
                || url.contains("scope=openid%20profile%20email")
        );
        // The verifier itself must never leave the device.
        assert!(!url.contains(&pending().verifier));
    }

    #[test]
    fn parse_callback_extracts_code_and_state() {
        assert_eq!(
            parse_callback("xyz.tdbm.chaos://auth/callback?code=abc123&state=state-123"),
            Some(("abc123".into(), "state-123".into()))
        );
        // Order must not matter, and extra parameters are ignored.
        assert_eq!(
            parse_callback("xyz.tdbm.chaos://auth/callback?state=s&code=c&iss=whatever"),
            Some(("c".into(), "s".into()))
        );
    }

    #[test]
    fn parse_callback_rejects_anything_else() {
        for url in [
            "xyz.tdbm.chaos://auth/callback?code=abc",    // no state
            "xyz.tdbm.chaos://auth/callback?state=abc",   // no code
            "xyz.tdbm.chaos://other/path?code=a&state=b", // not our path
            "https://evil.example/auth/callback?code=a&state=b", // not our scheme
            "not a url",
            "",
        ] {
            assert_eq!(parse_callback(url), None, "should have rejected {url}");
        }
    }

    #[test]
    fn needs_refresh_respects_the_margin() {
        let now = 1_000_000;
        assert!(needs_refresh(now - 1, now), "already expired");
        assert!(needs_refresh(now, now), "expiring exactly now");
        assert!(
            needs_refresh(now + REFRESH_MARGIN_SECS - 1, now),
            "inside the margin"
        );
        assert!(
            !needs_refresh(now + REFRESH_MARGIN_SECS + 1, now),
            "outside the margin"
        );
        assert!(!needs_refresh(now + 3600, now), "fresh token");
    }
}
