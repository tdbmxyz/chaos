//! Shared HTTP plumbing for the direct-fetch modules (`open_meteo`,
//! `posts`): a GET-JSON helper with a per-request deadline.

use std::time::Duration;

/// Per-request deadline: an unreachable upstream must fail the widget
/// fast, not hang "Loading" for minutes.
const TIMEOUT: Duration = Duration::from_secs(8);

/// GET a JSON document with the module's per-request deadline. Mirrors the
/// timeout pattern in `ChaosClient::check_status` (reqwest's builder-level
/// `.timeout()` isn't available on wasm; the request-level one is).
pub(crate) async fn http_get_json<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    url: &str,
) -> Result<T, String> {
    let mut request = http.get(url).build().map_err(|e| e.to_string())?;
    *request.timeout_mut() = Some(TIMEOUT);
    let response = http.execute(request).await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status().as_u16()));
    }
    response.json().await.map_err(|e| e.to_string())
}

/// GET a document as text with the module's per-request deadline. Used by
/// the direct comment-thread path, which parses the body itself (mapping
/// bodies to plain text) rather than deserializing a fixed shape.
pub(crate) async fn http_get_text(http: &reqwest::Client, url: &str) -> Result<String, String> {
    let mut request = http.get(url).build().map_err(|e| e.to_string())?;
    *request.timeout_mut() = Some(TIMEOUT);
    let response = http.execute(request).await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status().as_u16()));
    }
    response.text().await.map_err(|e| e.to_string())
}
