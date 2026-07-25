//! Static frontend serving: precompressed assets and their cache policy.

use std::path::Path;

use axum::Router;
use axum::extract::Request;
use axum::http::HeaderValue;
use axum::http::header::{CACHE_CONTROL, VARY};
use axum::middleware::Next;
use axum::response::Response;
use tower::ServiceBuilder;
use tower_http::services::{ServeDir, ServeFile};

/// A year — the longest max-age browsers honour in practice. Safe only for
/// content-hashed filenames, where a change means a new URL.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// Revalidate every time (a 304 is cheap; a stale `index.html` is not).
const REVALIDATE: &str = "no-cache";

/// Cache policy for a request path.
///
/// Trunk fingerprints the assets it builds — `styles-<16 hex>.css`,
/// `chaos-web-<16 hex>_bg.wasm` — so a changed file always arrives under a new
/// URL and can be pinned forever. Hand-vendored files (`/vendor/echarts.min.js`),
/// `/assets/*`, `index.html` and SPA routes carry no hash and must revalidate.
///
/// The test is "some `-`/`_`/`.`-delimited segment of the file name is exactly
/// 16 hex digits". A hand-written file that happens to match would be pinned
/// for a year; nothing in `crates/chaos-web` does.
pub(crate) fn cache_control_for(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path);
    let hashed = name
        .split(['-', '_', '.'])
        .any(|seg| seg.len() == 16 && seg.chars().all(|c| c.is_ascii_hexdigit()));
    if hashed { IMMUTABLE } else { REVALIDATE }
}

/// Stamp the cache policy for this path, plus the `Vary` that keeps a shared
/// cache from handing a brotli body to a client that never asked for one
/// (`ServeDir` sets `Content-Encoding` but not `Vary`).
async fn cache_headers(request: Request, next: Next) -> Response {
    let policy = cache_control_for(request.uri().path());
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static(policy));
    headers.insert(VARY, HeaderValue::from_static("accept-encoding"));
    response
}

/// Serves the built frontend out of `dir`, preferring the `.br`/`.gz` siblings
/// that `packages.chaos-web-static` generates at build time (see flake.nix) and
/// falling back to the identity file when they are absent — which is the case
/// for a local `just build-web` dist. Unknown paths get `index.html` so the
/// client-side router owns deep links.
pub(crate) fn router(dir: &Path) -> Router {
    let index = ServeFile::new(dir.join("index.html"))
        .precompressed_br()
        .precompressed_gzip();
    let files = ServeDir::new(dir)
        .precompressed_br()
        .precompressed_gzip()
        .fallback(index);

    Router::new().fallback_service(
        ServiceBuilder::new()
            .layer(axum::middleware::from_fn(cache_headers))
            .service(files),
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;

    use super::*;

    /// A throwaway dist directory: one fingerprinted wasm with a `.br`
    /// sibling, one without, and an index. Named after the calling test so
    /// tests never share a directory.
    fn dist(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("chaos-static-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create dist dir");
        fs::write(dir.join("index.html"), "<html>index</html>").unwrap();
        fs::write(dir.join("app-0123456789abcdef_bg.wasm"), "identity-wasm").unwrap();
        fs::write(
            dir.join("app-0123456789abcdef_bg.wasm.br"),
            "brotli-wasm-bytes",
        )
        .unwrap();
        fs::write(dir.join("styles-fedcba9876543210.css"), "body{}").unwrap();
        dir
    }

    async fn get(
        dir: &Path,
        path: &str,
        accept_encoding: Option<&str>,
    ) -> axum::response::Response {
        let mut req = Request::builder().uri(path);
        if let Some(encoding) = accept_encoding {
            req = req.header(header::ACCEPT_ENCODING, encoding);
        }
        router(dir)
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .expect("router is infallible")
    }

    async fn body_string(response: axum::response::Response) -> String {
        let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn brotli_capable_clients_get_the_precompressed_sibling() {
        let dir = dist("br-sibling");
        let response = get(&dir, "/app-0123456789abcdef_bg.wasm", Some("br")).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "br");
        assert_eq!(
            response.headers()[header::VARY]
                .to_str()
                .unwrap()
                .to_ascii_lowercase(),
            "accept-encoding"
        );
        assert_eq!(body_string(response).await, "brotli-wasm-bytes");
    }

    #[tokio::test]
    async fn clients_without_brotli_get_the_identity_file() {
        let dir = dist("identity");
        let response = get(&dir, "/app-0123456789abcdef_bg.wasm", None).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
        assert_eq!(body_string(response).await, "identity-wasm");
    }

    /// No `.br` sibling (a plain local `trunk build` dist): serve the original
    /// rather than 404.
    #[tokio::test]
    async fn missing_sibling_falls_back_to_the_identity_file() {
        let dir = dist("no-sibling");
        let response = get(&dir, "/styles-fedcba9876543210.css", Some("br, gzip")).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
        assert_eq!(body_string(response).await, "body{}");
    }

    #[tokio::test]
    async fn fingerprinted_assets_are_served_immutable() {
        let dir = dist("immutable-header");
        let response = get(&dir, "/app-0123456789abcdef_bg.wasm", Some("br")).await;

        assert_eq!(response.headers()[header::CACHE_CONTROL], IMMUTABLE);
    }

    /// SPA routes fall through to index.html, which must never be pinned.
    #[tokio::test]
    async fn spa_routes_serve_index_and_revalidate() {
        let dir = dist("spa-fallback");
        let response = get(&dir, "/links", None).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], REVALIDATE);
        assert_eq!(body_string(response).await, "<html>index</html>");
    }

    #[test]
    fn trunk_fingerprinted_assets_are_immutable() {
        assert_eq!(
            cache_control_for("/chaos-web-1243ba43bf8faa7b_bg.wasm"),
            IMMUTABLE
        );
        assert_eq!(
            cache_control_for("/chaos-web-1243ba43bf8faa7b.js"),
            IMMUTABLE
        );
        assert_eq!(cache_control_for("/styles-3e1677dddc3dd7f1.css"), IMMUTABLE);
    }

    #[test]
    fn unhashed_assets_and_spa_routes_revalidate() {
        assert_eq!(cache_control_for("/index.html"), REVALIDATE);
        assert_eq!(cache_control_for("/"), REVALIDATE);
        assert_eq!(cache_control_for("/links"), REVALIDATE);
        assert_eq!(cache_control_for("/vendor/echarts.min.js"), REVALIDATE);
        assert_eq!(cache_control_for("/assets/logo.svg"), REVALIDATE);
        assert_eq!(cache_control_for("/assets/favicon-32.png"), REVALIDATE);
        assert_eq!(cache_control_for("/assets/manifest.json"), REVALIDATE);
    }

    /// A uuid in a path must not read as a fingerprint: its groups are 8, 4, 4,
    /// 4 and 12 hex digits, never 16.
    #[test]
    fn uuid_paths_are_not_mistaken_for_fingerprints() {
        assert_eq!(
            cache_control_for("/api/v1/links/019f388b-4c21-7b3a-9f10-2d4e6a8c1b55"),
            REVALIDATE
        );
    }
}
