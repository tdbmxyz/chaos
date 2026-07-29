# App OIDC Authentication — Plan B (app + shell)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One tap signs the app in through authentik's own login page, the session survives app updates and offline periods, and the app-password path disappears.

**Architecture:** The authorization-code + PKCE exchange and every refresh happen in the Tauri Rust layer (`chaos-desktop`), never in JavaScript — authentik will not send CORS headers for `tauri://localhost` on a custom-scheme provider, and the refresh token then never touches WebView storage. `tauri-plugin-deep-link` catches `xyz.tdbm.chaos://auth/callback`; `tauri-plugin-store` persists tokens in the app data directory, which survives updates. `chaos-ui` learns what to do from `/api/v1/health`'s new `auth` block, polls the shell for an access token, and mirrors only the short-lived access token into `localStorage` so `use_client()` stays synchronous.

**Tech Stack:** Tauri 2 (deep-link, store, single-instance), Leptos 0.8 CSR, `reqwest` (native side), `sha2`/`base64`/`rand` for PKCE, `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-07-25-app-oidc-auth-design.md` (sections D, E, F)
**Depends on:** Plan A, complete — `/api/v1/health` advertises `auth.oidc{issuer, client_id, authorize_url}` and every route requires auth.

---

## Context for every task

- Repo root `/projects/rust/chaos`, branch `feat/app-oidc-auth` (already checked out). Work inside `nix develop` / direnv.
- **Commits:** unsigned, with both trailers:
  ```bash
  git -c commit.gpgsign=false commit -m "type: subject" \
    -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
    -m "Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP"
  ```
  Do not push. Never commit or edit anything under `/etc/nixos` (that is the user's).
- **Never touch** `crates/chaos-desktop/gen/schemas/{android,mobile}-schema.json`. A build may regenerate them; restore with `git checkout -- crates/chaos-desktop/gen/schemas/`.
- After any Rust change: `just check` and `just test` must pass before committing. `chaos-ui` compiles for **both** wasm and native (clippy runs `--all-targets`), so browser-only code must sit behind `wasm_bindgen` imports rather than direct DOM calls in shared paths.
- `just build-web` and `just apk` take several minutes each; let them finish.
- Existing patterns: `crates/chaos-ui/src/lib.rs`'s `open_external()` shows how the UI reaches `window.__TAURI__.core.invoke` and the Android `ChaosAndroid` bridge; `crates/chaos-desktop/src/lib.rs` shows how a `#[tauri::command]` is registered.

## File structure

| File | Responsibility |
| --- | --- |
| `crates/chaos-desktop/src/auth.rs` (new) | PKCE, discovery, code exchange, refresh, token storage, the four commands. All OIDC secrets live here and nowhere else. |
| `crates/chaos-desktop/src/lib.rs` | Plugin registration, deep-link handler, command registration. |
| `crates/chaos-desktop/tauri.conf.json` | deep-link scheme. |
| `crates/chaos-desktop/capabilities/default.json` | permissions for the new plugins. |
| `crates/chaos-desktop/gen/android/app/src/main/AndroidManifest.xml` | intent-filter for the callback scheme. |
| `flake.nix` | `MimeType=x-scheme-handler/xyz.tdbm.chaos;` on the Linux desktop entry. |
| `crates/chaos-ui/src/auth.rs` (new) | The UI half: invoke bridge, access-token mirror, sign-in/poll/sign-out. |
| `crates/chaos-ui/src/lib.rs` | Three-state `ServerGate`, session lifetime rules, removal of the app-password path. |
| `crates/chaos-ui/src/pages/settings.rs` | Authentik credential fields removed. |
| `crates/chaos-client/src/lib.rs` | `with_basic_auth` removed. |

---

### Task 1: PKCE and the token lifecycle in the shell

Pure functions first (they hold the security-relevant logic and are testable
without Tauri), then the storage and commands around them.

**Files:**
- Create: `crates/chaos-desktop/src/auth.rs`
- Modify: `crates/chaos-desktop/Cargo.toml`

- [ ] **Step 1: Add dependencies**

In `crates/chaos-desktop/Cargo.toml` under `[dependencies]`:

```toml
tauri-plugin-deep-link = "2"
tauri-plugin-store = "2"
reqwest = { workspace = true }
serde.workspace = true
serde_json.workspace = true
sha2 = "0.10"
base64 = "0.22"
rand = "0.8"
```

and, desktop-only, below the dependencies:

```toml
[target.'cfg(not(any(target_os = "android", target_os = "ios")))'.dependencies]
tauri-plugin-single-instance = "2"
```

Run: `cargo check -p chaos-desktop`
Expected: compiles.

- [ ] **Step 2: Write the failing tests**

Create `crates/chaos-desktop/src/auth.rs` with the pure functions unimplemented
and their tests:

```rust
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
    unimplemented!("code_challenge")
}

/// A fresh high-entropy URL-safe string for a verifier or a state value.
pub fn random_token() -> String {
    unimplemented!("random_token")
}

/// The full authorization URL to open in the system browser.
pub fn authorize_url(authorization_endpoint: &str, pending: &Pending) -> String {
    unimplemented!("authorize_url")
}

/// Pull `code` and `state` out of a deep-link callback URL. `None` when this
/// isn't a callback we started (wrong scheme/path, or missing parameters).
pub fn parse_callback(url: &str) -> Option<(String, String)> {
    unimplemented!("parse_callback")
}

/// Whether a stored access token needs refreshing, given its expiry and now
/// (both unix seconds).
pub fn needs_refresh(expires_at: i64, now: i64) -> bool {
    unimplemented!("needs_refresh")
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
        assert!(url.contains("scope=openid+profile+email") || url.contains("scope=openid%20profile%20email"));
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
            "xyz.tdbm.chaos://auth/callback?code=abc",           // no state
            "xyz.tdbm.chaos://auth/callback?state=abc",          // no code
            "xyz.tdbm.chaos://other/path?code=a&state=b",        // not our path
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
```

Register the module in `crates/chaos-desktop/src/lib.rs`, above `run()`:

```rust
mod auth;
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo nextest run -p chaos-desktop auth::`
Expected: 6 tests run, all FAIL with `not implemented`.

- [ ] **Step 4: Implement the pure functions**

Replace the bodies in `crates/chaos-desktop/src/auth.rs`:

```rust
pub fn code_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub fn random_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    // 32 bytes → 43 base64url characters, the RFC's minimum verifier length.
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn authorize_url(authorization_endpoint: &str, pending: &Pending) -> String {
    let challenge = code_challenge(&pending.verifier);
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("response_type", "code")
        .append_pair("client_id", &pending.client_id)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("scope", "openid profile email")
        .append_pair("state", &pending.state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .finish();
    format!("{authorization_endpoint}?{query}")
}

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

pub fn needs_refresh(expires_at: i64, now: i64) -> bool {
    expires_at - now <= REFRESH_MARGIN_SECS
}
```

`form_urlencoded` comes with `url`, which is already a dependency; add
`use` nothing — reference it as `url::form_urlencoded` if the bare path does not
resolve.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo nextest run -p chaos-desktop auth::`
Expected: 6 tests pass.

- [ ] **Step 6: Add discovery, exchange, refresh and storage**

Append to `crates/chaos-desktop/src/auth.rs`:

```rust
use tauri::{AppHandle, Manager, Runtime};
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

async fn discover(issuer: &str) -> Result<Discovery, String> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    reqwest::Client::new()
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
    store.set(KEY_ACCESS, tokens.access_token.clone().into());
    store.set(
        KEY_EXPIRES,
        (now() + tokens.expires_in.unwrap_or(3600)).into(),
    );
    if let Some(refresh) = &tokens.refresh_token {
        store.set(KEY_REFRESH, refresh.clone().into());
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
    store.set(KEY_ISSUER, issuer.into());
    store.set(KEY_CLIENT, client_id.into());
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
    let response = reqwest::Client::new()
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
        return Err(format!("identity provider rejected the code ({status}): {body}"));
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
    let access = store.get(KEY_ACCESS).and_then(|v| v.as_str().map(String::from));
    let expires_at = store.get(KEY_EXPIRES).and_then(|v| v.as_i64()).unwrap_or(0);
    if let Some(access) = access
        && !needs_refresh(expires_at, now())
    {
        return Ok(Some(access));
    }
    let (Some(refresh), Some(issuer), Some(client_id)) = (
        store.get(KEY_REFRESH).and_then(|v| v.as_str().map(String::from)),
        store.get(KEY_ISSUER).and_then(|v| v.as_str().map(String::from)),
        store.get(KEY_CLIENT).and_then(|v| v.as_str().map(String::from)),
    ) else {
        return Ok(None);
    };
    let discovery = discover(&issuer).await?;
    let response = reqwest::Client::new()
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

fn sign_out_store<R: Runtime>(store: &std::sync::Arc<tauri_plugin_store::Store<R>>) -> Result<(), String> {
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
```

If `tauri_plugin_store`'s API in the installed version differs (for example
`app.store(...)` returning a different handle type, or `set` taking
`impl Into<JsonValue>` differently), adapt minimally and report what changed —
do not weaken the state check or the storage location.

- [ ] **Step 7: Verify it compiles and the tests still pass**

Run: `cargo nextest run -p chaos-desktop`
Expected: 6 tests pass.

Run: `just check`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add crates/chaos-desktop/src/auth.rs crates/chaos-desktop/src/lib.rs \
        crates/chaos-desktop/Cargo.toml Cargo.lock
git -c commit.gpgsign=false commit -m "feat(shell): PKCE sign-in and durable token storage" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP"
```

---

### Task 2: Register the callback scheme end to end

**Files:**
- Modify: `crates/chaos-desktop/src/lib.rs`
- Modify: `crates/chaos-desktop/tauri.conf.json`
- Modify: `crates/chaos-desktop/capabilities/default.json`
- Modify: `crates/chaos-desktop/gen/android/app/src/main/AndroidManifest.xml`
- Modify: `flake.nix`

- [ ] **Step 1: Register the plugins and the deep-link handler**

In `crates/chaos-desktop/src/lib.rs`, extend the builder in `run()`:

```rust
    tauri::Builder::default()
        // Native HTTP for the UI (`window.__TAURI__.http.fetch`): lets it
        // reach hosts that send no CORS headers (lobste.rs). Scoped to an
        // explicit URL allowlist in capabilities/default.json.
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_deep_link::init())
        .invoke_handler(tauri::generate_handler![
            open_external,
            auth::auth_start,
            auth::auth_token,
            auth::auth_sign_out
        ])
```

and, before `.invoke_handler(...)`, the desktop-only single-instance plugin so a
callback URL reaches the running app instead of starting a second one:

```rust
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
        // A second launch carries the callback URL in argv; hand it to the
        // running instance rather than opening another window.
        for arg in argv {
            handle_callback(app, &arg);
        }
    }));
```

(Restructure `run()` to bind `let builder = tauri::Builder::default()…` so the
conditional plugin can be added; keep the rest of the chain intact.)

In `.setup(...)`, after the window is built, subscribe to deep links:

```rust
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        handle_callback(&handle, url.as_str());
                    }
                });
            }
```

and add the shared handler next to `open_external`:

```rust
/// Complete a sign-in from a callback URL, wherever it arrived from (a deep
/// link on Android, argv on the desktop). Errors are logged rather than
/// surfaced: the UI is polling `auth_token` and shows its own timeout.
fn handle_callback<R: tauri::Runtime>(app: &tauri::AppHandle<R>, url: &str) {
    let Some((code, state)) = auth::parse_callback(url) else {
        return;
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = auth::finish(&app, &code, &state).await {
            eprintln!("chaos: sign-in callback failed: {err}");
        }
    });
}
```

- [ ] **Step 2: Declare the scheme for the desktop**

In `crates/chaos-desktop/tauri.conf.json`, add a `plugins` block as a sibling of
`bundle`:

```json
  "plugins": {
    "deep-link": {
      "desktop": {
        "schemes": ["xyz.tdbm.chaos"]
      }
    }
  }
```

- [ ] **Step 3: Grant the plugin permissions**

In `crates/chaos-desktop/capabilities/default.json`, add to `permissions`:

```json
    "store:default",
    "deep-link:default",
```

- [ ] **Step 4: Add the Android intent-filter**

In `crates/chaos-desktop/gen/android/app/src/main/AndroidManifest.xml`, inside
the existing `<activity …android:name=".MainActivity"…>` element and after the
existing `<intent-filter>` blocks, add:

```xml
            <!-- authentik sends the browser back here after sign-in; the
                 scheme must match REDIRECT_URI in chaos-desktop/src/auth.rs
                 and the provider's redirect URI in authentik. -->
            <intent-filter>
                <action android:name="android.intent.action.VIEW" />
                <category android:name="android.intent.category.DEFAULT" />
                <category android:name="android.intent.category.BROWSABLE" />
                <data android:scheme="xyz.tdbm.chaos" android:host="auth" />
            </intent-filter>
```

- [ ] **Step 5: Register the scheme handler on Linux**

In `flake.nix`, in the `chaos-desktop` `postInstall` that writes
`$out/share/applications/chaos.desktop`, add a `MimeType` line to the
`[Desktop Entry]` block, after `Categories=Utility;`:

```
        MimeType=x-scheme-handler/xyz.tdbm.chaos;
```

- [ ] **Step 6: Verify it builds for both targets**

```bash
just check
cargo check -p chaos-desktop
git add -A crates/chaos-desktop flake.nix
nix build .#chaos-desktop
```

Expected: all succeed. If `nix build` fails on a missing plugin permission,
the message names the permission — add it to `capabilities/default.json`.

Confirm `crates/chaos-desktop/gen/schemas/{android,mobile}-schema.json` are
unchanged (`git status --porcelain crates/chaos-desktop/gen/schemas/` must be
empty); restore them if the build rewrote them.

- [ ] **Step 7: Commit**

```bash
git add crates/chaos-desktop flake.nix
git -c commit.gpgsign=false commit -m "feat(shell): register the xyz.tdbm.chaos callback scheme" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP"
```

---

### Task 3: The UI half — sign-in, token mirror, three-state gate

**Files:**
- Create: `crates/chaos-ui/src/auth.rs`
- Modify: `crates/chaos-ui/src/lib.rs`
- Modify: `crates/chaos-ui/src/offline.rs`

- [ ] **Step 1: Write the failing tests for the pure parts**

Create `crates/chaos-ui/src/auth.rs`:

```rust
//! The UI half of OIDC sign-in: talks to the shell's auth commands, mirrors
//! the short-lived access token where `use_client()` can read it
//! synchronously, and decides which gate state to show.
//!
//! The refresh token is deliberately NOT mirrored — it stays in the shell's
//! store, out of reach of anything running in the webview.

use chaos_domain::api::HealthResponse;

/// Where the access token is mirrored for `use_client()`. Short-lived and
/// re-fetchable, unlike the refresh token.
pub(crate) const ACCESS_TOKEN_KEY: &str = "chaos-oidc-access";

/// What the gate should show.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum GateState {
    Checking,
    Ready,
    NeedsSignIn,
    Unreachable,
}

/// The gate decision, as a pure function of everything that matters.
///
/// `health` is the probe result: `Some(response)` when the server answered.
/// `has_token` is whether the app holds an access token. `seen` is whether
/// this server has ever answered before — a known server that is merely
/// offline boots into the cached UI rather than a connect form.
pub(crate) fn gate_state(
    health: Option<&HealthResponse>,
    has_token: bool,
    seen: bool,
) -> GateState {
    unimplemented!("gate_state")
}

#[cfg(test)]
mod tests {
    use chaos_domain::api::{AuthAdvertisement, OidcAdvertisement};

    use super::*;

    fn health(oidc: bool) -> HealthResponse {
        HealthResponse {
            status: "ok".into(),
            version: "1.12.0".into(),
            fahrenheit: None,
            auth: oidc.then(|| AuthAdvertisement {
                oidc: Some(OidcAdvertisement {
                    issuer: "https://auth.example/application/o/chaos-app/".into(),
                    client_id: "client-abc".into(),
                    authorize_url: "https://auth.example/application/o/authorize/".into(),
                }),
            }),
        }
    }

    #[test]
    fn a_server_without_oidc_is_ready_without_a_token() {
        assert_eq!(gate_state(Some(&health(false)), false, false), GateState::Ready);
    }

    #[test]
    fn a_server_with_oidc_needs_sign_in_until_a_token_is_held() {
        assert_eq!(
            gate_state(Some(&health(true)), false, false),
            GateState::NeedsSignIn
        );
        assert_eq!(gate_state(Some(&health(true)), true, false), GateState::Ready);
    }

    /// Offline with a server we've reached before: show the cached app and the
    /// offline badge, never a sign-in or connect form we can't act on.
    #[test]
    fn a_known_server_that_is_offline_stays_ready() {
        assert_eq!(gate_state(None, false, true), GateState::Ready);
        assert_eq!(gate_state(None, true, true), GateState::Ready);
    }

    #[test]
    fn an_unknown_server_that_never_answered_is_unreachable() {
        assert_eq!(gate_state(None, false, false), GateState::Unreachable);
        assert_eq!(gate_state(None, true, false), GateState::Unreachable);
    }
}
```

Register it in `crates/chaos-ui/src/lib.rs` next to the other `mod` lines:

```rust
pub(crate) mod auth;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p chaos-ui auth::`
Expected: 4 tests run, all FAIL with `not implemented: gate_state`.

- [ ] **Step 3: Implement `gate_state`**

```rust
pub(crate) fn gate_state(
    health: Option<&HealthResponse>,
    has_token: bool,
    seen: bool,
) -> GateState {
    let Some(health) = health else {
        // The probe failed. A server we've reached before is just offline
        // right now; one we've never reached is misconfigured.
        return if seen {
            GateState::Ready
        } else {
            GateState::Unreachable
        };
    };
    let wants_oidc = health
        .auth
        .as_ref()
        .and_then(|auth| auth.oidc.as_ref())
        .is_some();
    if wants_oidc && !has_token {
        GateState::NeedsSignIn
    } else {
        GateState::Ready
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p chaos-ui auth::`
Expected: 4 tests pass.

- [ ] **Step 5: Add the shell bridge**

Append to `crates/chaos-ui/src/auth.rs`:

```rust
use leptos::prelude::*;
use wasm_bindgen::prelude::*;

/// Invoke a shell command and await its result. `Err` in a plain browser,
/// where `__TAURI__` doesn't exist — the web build never signs in this way,
/// it rides the browser's own authentik session.
async fn invoke(command: &str, args: JsValue) -> Result<JsValue, JsValue> {
    use wasm_bindgen::JsCast;
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let tauri = js_sys::Reflect::get(&window, &"__TAURI__".into())?;
    if tauri.is_undefined() {
        return Err(JsValue::from_str("not running in a shell"));
    }
    let core = js_sys::Reflect::get(&tauri, &"core".into())?;
    let invoke: js_sys::Function = js_sys::Reflect::get(&core, &"invoke".into())?.dyn_into()?;
    let promise: js_sys::Promise = invoke
        .call2(&core, &command.into(), &args)?
        .dyn_into()?;
    wasm_bindgen_futures::JsFuture::from(promise).await
}

/// True when running inside a shell that can perform OIDC sign-in.
pub(crate) fn shell_available() -> bool {
    web_sys::window()
        .and_then(|w| js_sys::Reflect::get(&w, &"__TAURI__".into()).ok())
        .is_some_and(|t| !t.is_undefined())
}

/// The mirrored access token, read synchronously by `use_client()`.
pub(crate) fn access_token() -> Option<String> {
    crate::pref(ACCESS_TOKEN_KEY)
}

fn set_access_token(token: Option<&str>) {
    crate::set_pref(ACCESS_TOKEN_KEY, token.unwrap_or(""));
}

/// Ask the shell for a current access token (refreshing if needed) and mirror
/// it. Returns whether a token is held afterwards.
pub(crate) async fn sync_access_token() -> bool {
    match invoke("auth_token", JsValue::UNDEFINED).await {
        Ok(value) => {
            let token = value.as_string();
            set_access_token(token.as_deref());
            token.is_some()
        }
        // No shell (browser), or the command failed: leave any mirrored token
        // alone rather than signing the user out over a transient error.
        Err(_) => access_token().is_some(),
    }
}

/// Begin sign-in: ask the shell for the authorize URL and open it in the
/// system browser. The shell finishes the exchange when authentik redirects
/// back; the caller polls `sync_access_token`.
pub(crate) async fn start_sign_in(issuer: &str, client_id: &str) -> Result<(), String> {
    let args = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&args, &"issuer".into(), &issuer.into());
    let _ = js_sys::Reflect::set(&args, &"clientId".into(), &client_id.into());
    let url = invoke("auth_start", args.into())
        .await
        .map_err(|e| format!("{e:?}"))?
        .as_string()
        .ok_or("the shell returned no authorize URL")?;
    crate::open_external(&url);
    Ok(())
}

pub(crate) async fn sign_out() {
    let _ = invoke("auth_sign_out", JsValue::UNDEFINED).await;
    set_access_token(None);
}
```

Note the argument name: Tauri converts snake_case command parameters to
camelCase for JS callers, so `client_id` is passed as `clientId`.

- [ ] **Step 6: Make the client use the mirrored token**

In `crates/chaos-ui/src/lib.rs`, `use_client()` currently reads
`config.persist_token.then(stored_token).flatten()`. Prefer the OIDC token when
one is mirrored:

```rust
    // An OIDC access token (shells, signed in through authentik) wins over
    // chaos's own session token; the server accepts either.
    let token = crate::auth::access_token()
        .or_else(|| config.persist_token.then(stored_token).flatten());
```

Apply the same change to both branches of the `match use_context::<SharedClient>()`.

- [ ] **Step 7: Capture the advertisement in the probe**

In `crates/chaos-ui/src/offline.rs`, `probe()` already receives the whole
`HealthResponse`. Store it so the gate can read it:

```rust
        Ok(health) => {
            crate::set_server_fahrenheit(health.fahrenheit);
            cache_put("server-fahrenheit", &health.fahrenheit);
            // The gate needs the auth advertisement, and an offline boot needs
            // the last-known one so it doesn't forget the server wants OIDC.
            cache_put("server-auth", &health.auth);
            crate::auth::set_advertisement(health.auth.clone());
            mark_server_seen(client.base().as_str());
```

and add to `crates/chaos-ui/src/auth.rs`:

```rust
/// The server's advertised auth methods, from the last successful probe.
/// A signal so the gate re-renders when a probe answers.
pub(crate) fn advertisement() -> RwSignal<Option<chaos_domain::api::AuthAdvertisement>> {
    use_context::<RwSignal<Option<chaos_domain::api::AuthAdvertisement>>>()
        .expect("auth advertisement provided by App")
}

pub(crate) fn set_advertisement(value: Option<chaos_domain::api::AuthAdvertisement>) {
    if let Some(signal) =
        use_context::<RwSignal<Option<chaos_domain::api::AuthAdvertisement>>>()
    {
        signal.set(value);
    }
}
```

and provide it in `App`, next to the connectivity signal:

```rust
    let advertisement = RwSignal::new(offline::cache_get::<
        Option<chaos_domain::api::AuthAdvertisement>,
    >("server-auth").flatten());
    provide_context(advertisement);
```

- [ ] **Step 8: Make `ServerGate` three-state**

Replace the `ServerGate` component body in `crates/chaos-ui/src/lib.rs`. Keep
the existing probe/seen logic and the address form; add the sign-in state:

```rust
#[component]
fn ServerGate(children: ChildrenFn) -> impl IntoView {
    let gate = RwSignal::new(auth::GateState::Checking);
    let client = use_client();
    let conn = offline::use_connectivity();
    let seen = offline::server_seen(use_client().base().as_str());
    let advertisement = auth::advertisement();
    // "Waiting for the browser to come back" — drives the polling UI.
    let waiting = RwSignal::new(false);

    spawn_local(async move {
        // A shell may already hold a token from a previous run; mirror it
        // before the first gate decision so a signed-in app never flashes the
        // sign-in screen.
        let has_token = auth::sync_access_token().await;
        let healthy = offline::probe(&client, conn).await;
        if !healthy && seen {
            set_server_fahrenheit(offline::cache_get::<Option<bool>>("server-fahrenheit").flatten());
        }
        let health = healthy.then(|| chaos_domain::api::HealthResponse {
            status: "ok".into(),
            version: String::new(),
            fahrenheit: None,
            auth: advertisement.get_untracked(),
        });
        gate.set(auth::gate_state(health.as_ref(), has_token, seen));
    });

    let sign_in = move |_| {
        let Some(oidc) = advertisement
            .get_untracked()
            .and_then(|a| a.oidc)
        else {
            return;
        };
        waiting.set(true);
        spawn_local(async move {
            if auth::start_sign_in(&oidc.issuer, &oidc.client_id)
                .await
                .is_err()
            {
                waiting.set(false);
                return;
            }
            // The shell completes the exchange when authentik redirects back;
            // poll until a token appears. Two minutes is long enough for a
            // password + 2FA and short enough not to poll forever.
            for _ in 0..80 {
                gloo_timers::future::TimeoutFuture::new(1_500).await;
                if auth::sync_access_token().await {
                    waiting.set(false);
                    gate.set(auth::GateState::Ready);
                    return;
                }
            }
            waiting.set(false);
        });
    };

    let current = use_client().base().to_string();
    let input = RwSignal::new(current);
    let connect = move |_| {
        let value = input.get_untracked().trim().to_string();
        if Url::parse(&value).is_err() {
            return;
        }
        set_api_base_override(Some(&value));
    };

    view! {
        {move || match gate.get() {
            auth::GateState::Checking => {
                view! { <p class="muted gate-msg">"Connecting…"</p> }.into_any()
            }
            auth::GateState::Ready => children().into_any(),
            auth::GateState::NeedsSignIn => {
                view! {
                    <section class="server-gate">
                        <h2>"Sign in"</h2>
                        <p class="muted">
                            "This server is protected by authentik. Signing in opens it in your browser."
                        </p>
                        <div class="gate-form">
                            <button class="primary" on:click=sign_in disabled=move || waiting.get()>
                                {move || if waiting.get() { "Waiting for authentik…" } else { "Sign in with authentik" }}
                            </button>
                        </div>
                    </section>
                }
                    .into_any()
            }
            auth::GateState::Unreachable => {
                view! {
                    <section class="server-gate">
                        <h2>"Cannot reach the chaos server"</h2>
                        <p class="muted">
                            "Enter the address of your server (for example "
                            <code>"http://zeus:4600"</code> ")."
                        </p>
                        <div class="gate-form">
                            <input
                                type="url"
                                prop:value=move || input.get()
                                on:input=move |ev| input.set(event_target_value(&ev))
                            />
                            <button class="primary" on:click=connect>"Connect"</button>
                            <button on:click=move |_| gate.set(auth::GateState::Ready)>
                                "Continue anyway"
                            </button>
                        </div>
                    </section>
                }
                    .into_any()
            }
        }}
    }
}
```

`gloo-timers` may not be a dependency yet. Check
`crates/chaos-ui/Cargo.toml`; if it is absent, either add
`gloo-timers = { version = "0.3", features = ["futures"] }` or use the existing
`leptos::set_timeout` with a oneshot channel — whichever matches what the crate
already does elsewhere (grep for `set_timeout`).

- [ ] **Step 9: Fix the session lifetime rules**

In `crates/chaos-ui/src/lib.rs`, the session-restore effect currently drops the
session for **any** `ClientError::Api`. Narrow it to 401, and refresh the token
before giving up:

```rust
                // The server answered "no session" (expired/revoked): try one
                // token refresh before concluding the user is signed out — an
                // access token that expired while the app slept is normal.
                Err(chaos_client::ClientError::Api { status: 401, .. }) => {
                    if crate::auth::sync_access_token().await {
                        if let Ok(user) = use_client().me().await {
                            offline::cache_put("me", &user);
                            session.0.set(Some(user));
                            return;
                        }
                    }
                    offline::cache_remove("me");
                    session.0.set(None);
                }
                // Any other API error says nothing about the session.
                Err(chaos_client::ClientError::Api { .. }) => {}
```

- [ ] **Step 10: Sign out through the shell too**

In `use_logout()` in `crates/chaos-ui/src/lib.rs`, clear the OIDC tokens as well:

```rust
        spawn_local(async move {
            let _ = client.logout().await;
            crate::auth::sign_out().await;
            store_token(None);
            offline::cache_clear();
            session.0.set(None);
        });
```

- [ ] **Step 11: Verify**

Run: `just check`
Expected: clean, including the wasm target.

Run: `cargo nextest run -p chaos-ui`
Expected: all pass.

- [ ] **Step 12: Commit**

```bash
git add crates/chaos-ui/src/auth.rs crates/chaos-ui/src/lib.rs \
        crates/chaos-ui/src/offline.rs crates/chaos-ui/Cargo.toml Cargo.lock
git -c commit.gpgsign=false commit -m "feat(ui): sign in with authentik from the gate" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP"
```

---

### Task 4: Remove the app-password path

**Files:**
- Modify: `crates/chaos-ui/src/pages/settings.rs`
- Modify: `crates/chaos-ui/src/lib.rs`
- Modify: `crates/chaos-client/src/lib.rs`

- [ ] **Step 1: Delete the settings UI**

In `crates/chaos-ui/src/pages/settings.rs`, remove the "Authentik" section:
the `<h3>"Authentik"</h3>` block and its `settings-authentik` div, the
`ak_user`/`ak_token` signals, and the save/forget handlers that call
`set_authentik_creds`/`clear_authentik_creds`.

- [ ] **Step 2: Delete the storage helpers**

In `crates/chaos-ui/src/lib.rs`, remove `AUTHENTIK_USER_KEY`,
`AUTHENTIK_TOKEN_KEY`, `authentik_creds_from`, `authentik_creds`,
`set_authentik_creds`, `clear_authentik_creds`, their tests, and the
`.with_basic_auth(authentik_creds())` calls in `use_client()`.

- [ ] **Step 3: Delete the client method**

In `crates/chaos-client/src/lib.rs`, remove the `basic_auth` field,
`with_basic_auth`, `has_basic_auth`, the `basic_auth` arm of the request
builder match (leaving the token arm), and the `basic_auth_builder_sets_creds`
test.

The request builder match becomes just the token case — check the surrounding
code and simplify it to an `if let Some(token) = &self.token` rather than
leaving a one-armed match.

- [ ] **Step 4: Clean up stored credentials on upgrade**

Old installs have `chaos-authentik-user`/`chaos-authentik-token` in
`localStorage`. Leaving a password behind is untidy; remove them once, in `App`:

```rust
    // Migration: the app-password path was replaced by OIDC sign-in. Drop the
    // stored credentials rather than leaving a password in localStorage.
    for stale in ["chaos-authentik-user", "chaos-authentik-token"] {
        set_pref(stale, "");
    }
```

- [ ] **Step 5: Verify**

Run: `just check && just test`
Expected: both clean. Confirm nothing references the removed names:

```bash
grep -rn "authentik_creds\|with_basic_auth\|AUTHENTIK_USER_KEY\|AUTHENTIK_TOKEN_KEY" \
  crates/ --include="*.rs"
```

Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add crates/
git -c commit.gpgsign=false commit -m "refactor(app): drop the app-password path in favour of OIDC" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP"
```

---

### Task 5: Verify the gate against a real server, and build the APK

The full sign-in round trip cannot be tested here — it needs the authentik
provider, which only the user can create. Everything up to the browser handoff
can be.

- [ ] **Step 1: Run a server that advertises OIDC**

```bash
just build-web
cat > /tmp/claude-1000/-projects-rust-chaos/9ca81220-5389-43ca-b5d7-f9d2302562c0/scratchpad/oidc.toml <<'TOML'
listen = "127.0.0.1:4699"
db_path = "/tmp/claude-1000/-projects-rust-chaos/9ca81220-5389-43ca-b5d7-f9d2302562c0/scratchpad/oidc-test.db"
static_dir = "crates/chaos-web/dist"

[oidc]
issuer = "https://auth.zeus.balem.fr/application/o/chaos-app/"
client_id = "not-a-real-client"
TOML
CHAOS_CONFIG=/tmp/claude-1000/-projects-rust-chaos/9ca81220-5389-43ca-b5d7-f9d2302562c0/scratchpad/oidc.toml \
  cargo run -p chaos-server &
sleep 5
curl -s http://127.0.0.1:4699/api/v1/health
curl -s -o /dev/null -w "unauthenticated dashboard: %{http_code}\n" http://127.0.0.1:4699/api/v1/dashboard
```

Expected: the health body contains an `auth.oidc` block with that issuer and
client_id, and `/api/v1/dashboard` answers **401**.

- [ ] **Step 2: Confirm the browser build shows the sign-in gate**

With that server running, drive headless Firefox (`nix-shell -p geckodriver
firefox`, WebDriver over curl, as in earlier work on this branch) against
`http://127.0.0.1:4699`:

- `document.body.innerText` contains "Sign in" and "Sign in with authentik"
- it does **not** contain "Cannot reach the chaos server"

That proves the three-state gate distinguishes "needs sign-in" from
"unreachable" — the exact bug this project exists to fix. In a plain browser the
button cannot complete a sign-in (no shell), which is expected: the web build
rides the browser's own authentik session.

Kill the server afterwards.

- [ ] **Step 3: Build the APK**

Run: `just apk`
Expected: succeeds. Verify with `aapt2 dump badging` (from
`$ANDROID_HOME/build-tools/35.0.0/`) that the package is `xyz.tdbm.chaos` and
`versionName` is the workspace version, and that the manifest now carries the
callback intent-filter:

```bash
aapt2 dump xmltree --file AndroidManifest.xml <apk> | grep -A3 "xyz.tdbm.chaos"
```

Expected: a `scheme="xyz.tdbm.chaos"` `host="auth"` data element appears.

- [ ] **Step 4: Commit any build-driven changes**

`git status --porcelain` — if the build regenerated
`crates/chaos-desktop/gen/schemas/{android,mobile}-schema.json`, restore them
(`git checkout -- crates/chaos-desktop/gen/schemas/`) rather than committing
them. Commit anything else that legitimately changed.

---

### Task 6: The deployment checklist

- [ ] **Step 1: Write it**

Create `docs/oidc-rollout.md` with the exact steps the user performs, in order,
each with what to expect. Cover:

1. **authentik** — create the OAuth2/OIDC provider (public client, PKCE S256
   required, redirect URI `xyz.tdbm.chaos://auth/callback`, **RSA signing key**,
   scopes `openid profile email`, access token ~hours, refresh token ~months),
   bind it to an application, and copy the client_id. Note that the issuer is
   shown on the provider page and ends with a slash.
2. **Verify the provider before touching anything else** — 
   `curl -s <issuer>.well-known/openid-configuration | tr ',' '\n' | grep jwks_uri`
   then fetch that `jwks_uri`: it must contain a key, not `{}`. An empty JWKS
   means the signing key is not RSA and nothing will verify.
3. **chaos config** in `/etc/nixos` — the `[oidc]` block with issuer and
   client_id.
4. **traefik** — the two routers from the spec (Bearer bypass + unauthenticated
   `/api/v1/health`), noting they must land *after* the server update.
5. **`nixos-rebuild switch`**, then check
   `curl -s https://<domain>/api/v1/health` returns the advertisement without a
   redirect, and that `curl -s -o /dev/null -w '%{http_code}' https://<domain>/api/v1/dashboard`
   is 401 rather than a 302 to authentik.
6. **Install the APK**, tap "Sign in with authentik", confirm the browser opens
   authentik's login page and the app becomes usable on return.
7. **Rollback** — remove `[oidc]` and the two routers, rebuild; the browser path
   is untouched throughout, so the web UI keeps working at every step.

- [ ] **Step 2: Commit**

```bash
git add docs/oidc-rollout.md
git -c commit.gpgsign=false commit -m "docs: OIDC rollout checklist" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP"
```

---

## Self-review

**Spec coverage (sections D, E, F):**

| spec requirement | task |
| --- | --- |
| exchange + refresh in Rust, not the webview | 1 |
| PKCE S256, state check | 1 |
| tokens in `tauri-plugin-store` (survive updates) | 1 |
| `auth_start` / `auth_token` / `auth_sign_out` / callback finish | 1, 2 |
| deep link: Android intent-filter, desktop scheme, `.desktop` MimeType | 2 |
| access token mirrored for `use_client`, refresh token never mirrored | 3 |
| three-state gate | 3 |
| session survives offline expiry; cleared only on 401 after a failed refresh | 3 (steps 9, 10) |
| app-password path removed | 4 |
| stale stored credentials cleaned up | 4 |

**Known gap, deliberate:** the browser handoff and return cannot be verified in
this session — it needs the authentik provider. Task 5 verifies everything up to
that point (the gate state, the 401s, the intent-filter in the built APK), and
Task 6 hands the user an ordered checklist for the rest.

**Type consistency:** `GateState::{Checking, Ready, NeedsSignIn, Unreachable}`,
`gate_state(health, has_token, seen)`, `sync_access_token`, `start_sign_in`,
`sign_out`, `access_token`, `advertisement`/`set_advertisement`,
`auth_start`/`auth_token`/`auth_sign_out`, `auth::finish`, `parse_callback`,
`needs_refresh`, `REDIRECT_URI` are spelled identically everywhere they appear,
and `REDIRECT_URI` matches the Android intent-filter and the authentik redirect
URI in Task 6.
