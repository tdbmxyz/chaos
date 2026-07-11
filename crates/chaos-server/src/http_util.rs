//! Shared HTTP fetch helpers for widget providers and feed subscriptions.
//!
//! Two rules every remote fetch must follow: check the status before the
//! body, and never buffer an unbounded body — `get_body_capped` streams and
//! fails as soon as the cap is crossed, so a hostile or misconfigured URL
//! cannot balloon server memory.

use futures::StreamExt;

/// GET `url` and deserialize the JSON body. Non-2xx statuses become errors.
pub async fn get_json<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    url: &str,
) -> Result<T, String> {
    let resp = http.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("status {}", resp.status()));
    }
    resp.json().await.map_err(|e| e.to_string())
}

/// GET `url` and return the raw body, erroring as soon as it exceeds
/// `max_bytes` — the body is streamed, never buffered past the cap.
pub async fn get_body_capped(
    http: &reqwest::Client,
    url: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    let resp = http.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("status {}", resp.status()));
    }
    read_capped(resp.bytes_stream(), max_bytes).await
}

/// Accumulate a chunk stream, failing once the total would pass `max_bytes`.
/// Split out from `get_body_capped` so the cap logic is testable with a
/// synthetic stream.
async fn read_capped<S, B, E>(stream: S, max_bytes: usize) -> Result<Vec<u8>, String>
where
    S: futures::Stream<Item = Result<B, E>>,
    B: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let mut stream = std::pin::pin!(stream);
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("reading body: {e}"))?;
        let chunk = chunk.as_ref();
        if body.len() + chunk.len() > max_bytes {
            return Err(format!("body exceeds {max_bytes} bytes"));
        }
        body.extend_from_slice(chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Payload {
        answer: u32,
    }

    /// One-route stub server (pattern from home_assistant.rs tests):
    /// always answers `status` + `body`. Returns the full URL.
    async fn stub(status: axum::http::StatusCode, body: &'static str) -> String {
        let app = axum::Router::new().route(
            "/data",
            axum::routing::get(move || async move { (status, body) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding stub");
        let addr = listener.local_addr().expect("stub addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serving stub");
        });
        format!("http://{addr}/data")
    }

    #[tokio::test]
    async fn get_json_deserializes_a_success_response() {
        let url = stub(axum::http::StatusCode::OK, r#"{"answer":42}"#).await;
        let got: Payload = get_json(&reqwest::Client::new(), &url)
            .await
            .expect("json");
        assert_eq!(got, Payload { answer: 42 });
    }

    #[tokio::test]
    async fn get_json_reports_http_errors_before_parsing() {
        let url = stub(axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom").await;
        let err = get_json::<Payload>(&reqwest::Client::new(), &url)
            .await
            .expect_err("5xx must fail");
        assert!(err.contains("500"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn get_body_capped_returns_bodies_under_the_cap() {
        let url = stub(axum::http::StatusCode::OK, "hello").await;
        let body = get_body_capped(&reqwest::Client::new(), &url, 1024)
            .await
            .expect("body");
        assert_eq!(body, b"hello");
    }

    #[tokio::test]
    async fn get_body_capped_rejects_oversized_bodies() {
        let url = stub(axum::http::StatusCode::OK, "hello world").await;
        let err = get_body_capped(&reqwest::Client::new(), &url, 4)
            .await
            .expect_err("cap must trip");
        assert!(err.contains("exceeds 4 bytes"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn read_capped_stops_mid_stream_at_the_cap() {
        let chunks: Vec<Result<&[u8], String>> =
            vec![Ok(b"aaaa"), Ok(b"bbbb"), Ok(b"cccc")];
        let err = read_capped(futures::stream::iter(chunks), 6)
            .await
            .expect_err("third chunk crosses the cap on the second");
        assert!(err.contains("exceeds 6 bytes"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn read_capped_accumulates_streams_that_fit() {
        let chunks: Vec<Result<&[u8], String>> = vec![Ok(b"aaaa"), Ok(b"bb")];
        let body = read_capped(futures::stream::iter(chunks), 6)
            .await
            .expect("exactly at the cap is fine");
        assert_eq!(body, b"aaaabb");
    }

    #[tokio::test]
    async fn read_capped_surfaces_stream_errors() {
        let chunks: Vec<Result<&[u8], String>> =
            vec![Ok(b"aa"), Err("reset by peer".into())];
        let err = read_capped(futures::stream::iter(chunks), 1024)
            .await
            .expect_err("stream error must propagate");
        assert!(err.contains("reset by peer"), "unexpected error: {err}");
    }
}
