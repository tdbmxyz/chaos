//! The UI half of OIDC sign-in: talks to the shell's auth commands, mirrors
//! the short-lived access token where `use_client()` can read it
//! synchronously, and decides which gate state to show.
//!
//! The refresh token is deliberately NOT mirrored — it stays in the shell's
//! store, out of reach of anything running in the webview.

use chaos_domain::api::{AuthAdvertisement, HealthResponse};
use leptos::prelude::*;
use leptos::wasm_bindgen::JsCast;
use leptos::wasm_bindgen::JsValue;

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

// ---- the shell bridge ----

/// Invoke a shell command and await its result. `Err` in a plain browser,
/// where `__TAURI__` doesn't exist — the web build never signs in this way,
/// it rides the browser's own authentik session.
async fn invoke(command: &str, args: JsValue) -> Result<JsValue, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let tauri = js_sys::Reflect::get(&window, &"__TAURI__".into())?;
    if tauri.is_undefined() {
        return Err(JsValue::from_str("not running in a shell"));
    }
    let core = js_sys::Reflect::get(&tauri, &"core".into())?;
    let invoke: js_sys::Function = js_sys::Reflect::get(&core, &"invoke".into())?.dyn_into()?;
    let promise: js_sys::Promise = invoke.call2(&core, &command.into(), &args)?.dyn_into()?;
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
    sync_status().await.0
}

/// Like [`sync_access_token`], but also returns what the shell says the
/// sign-in flow last did. A phone has no console, so a sign-in that fails
/// silently is indistinguishable from one that hangs — the gate shows this.
pub(crate) async fn sync_status() -> (bool, Option<String>) {
    match invoke("auth_status", JsValue::UNDEFINED).await {
        Ok(value) => {
            let token = js_sys::Reflect::get(&value, &"token".into())
                .ok()
                .and_then(|v| v.as_string());
            let status = js_sys::Reflect::get(&value, &"status".into())
                .ok()
                .and_then(|v| v.as_string());
            set_access_token(token.as_deref());
            (token.is_some(), status)
        }
        // No shell (browser), or the command failed: leave any mirrored token
        // alone rather than signing the user out over a transient error.
        Err(_) => (access_token().is_some(), None),
    }
}

/// Begin sign-in: ask the shell for the authorize URL and open it in the
/// system browser. The shell finishes the exchange when authentik redirects
/// back; the caller polls `sync_access_token`.
pub(crate) async fn start_sign_in(issuer: &str, client_id: &str) -> Result<(), String> {
    let args = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&args, &"issuer".into(), &issuer.into());
    // Tauri converts snake_case command parameters to camelCase for JS
    // callers, so `client_id` is passed as `clientId`.
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

// ---- the server's advertised auth methods ----

/// The server's advertised auth methods, from the last successful probe.
/// A signal so the gate re-renders when a probe answers.
pub(crate) fn advertisement() -> RwSignal<Option<AuthAdvertisement>> {
    use_context::<RwSignal<Option<AuthAdvertisement>>>()
        .expect("auth advertisement provided by App")
}

pub(crate) fn set_advertisement(value: Option<AuthAdvertisement>) {
    if let Some(signal) = use_context::<RwSignal<Option<AuthAdvertisement>>>() {
        signal.set(value);
    }
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
        assert_eq!(
            gate_state(Some(&health(false)), false, false),
            GateState::Ready
        );
    }

    #[test]
    fn a_server_with_oidc_needs_sign_in_until_a_token_is_held() {
        assert_eq!(
            gate_state(Some(&health(true)), false, false),
            GateState::NeedsSignIn
        );
        assert_eq!(
            gate_state(Some(&health(true)), true, false),
            GateState::Ready
        );
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
