//! Native-fetch bridge: `window.__TAURI__.http.fetch` routes through the
//! shell's Rust HTTP client, sidestepping webview CORS. Only used for hosts
//! that don't send CORS headers (lobste.rs); scoped by the shell's
//! capability file to an explicit allowlist.

use leptos::wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

fn fetch_fn() -> Option<js_sys::Function> {
    let window = web_sys::window()?;
    let tauri = js_sys::Reflect::get(&window, &"__TAURI__".into()).ok()?;
    if tauri.is_undefined() {
        return None;
    }
    let http = js_sys::Reflect::get(&tauri, &"http".into()).ok()?;
    js_sys::Reflect::get(&http, &"fetch".into())
        .ok()?
        .dyn_into()
        .ok()
}

/// GET `url` through the shell and return the body text. `None` when no
/// plugin is available (plain browser); `Some(Err)` on request failure.
pub(crate) async fn fetch_text(url: &str) -> Option<Result<String, String>> {
    let fetch = fetch_fn()?;
    Some(fetch_text_inner(&fetch, url).await)
}

async fn fetch_text_inner(fetch: &js_sys::Function, url: &str) -> Result<String, String> {
    let promise: js_sys::Promise = fetch
        .call1(&leptos::wasm_bindgen::JsValue::UNDEFINED, &url.into())
        .map_err(|e| format!("{e:?}"))?
        .dyn_into()
        .map_err(|_| "fetch did not return a promise".to_string())?;
    let response: web_sys::Response = JsFuture::from(promise)
        .await
        .map_err(|e| format!("{e:?}"))?
        .dyn_into()
        .map_err(|_| "not a Response".to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let text = JsFuture::from(response.text().map_err(|e| format!("{e:?}"))?)
        .await
        .map_err(|e| format!("{e:?}"))?;
    text.as_string().ok_or_else(|| "body was not text".into())
}
