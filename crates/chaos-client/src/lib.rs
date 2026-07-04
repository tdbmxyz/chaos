//! Typed client for the chaos HTTP API.
//!
//! Compiles on both native (tokio + rustls) and wasm (browser fetch) targets
//! thanks to reqwest's dual backend. All UI crates go through this client so
//! the API surface is exercised from exactly one place.

use chaos_domain::{ApiErrorBody, HealthResponse, ServiceWithStatus};
use url::Url;

/// Errors are stringly-typed on purpose: they cross into UI code that only
/// needs to display them, and `reqwest::Error` is neither `Clone` nor
/// available identically on wasm.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ClientError {
    #[error("request failed: {0}")]
    Transport(String),
    #[error("server returned {status}: {message}")]
    Api { status: u16, message: String },
    #[error("invalid response body: {0}")]
    Decode(String),
}

pub type Result<T> = std::result::Result<T, ClientError>;

#[derive(Debug, Clone)]
pub struct ChaosClient {
    base: Url,
    http: reqwest::Client,
}

impl ChaosClient {
    /// `base` is the server origin, e.g. `http://zeus:4600` — without the
    /// `/api/v1` prefix, which the client appends itself.
    pub fn new(base: Url) -> Self {
        Self {
            base,
            http: reqwest::Client::new(),
        }
    }

    pub fn base(&self) -> &Url {
        &self.base
    }

    pub async fn health(&self) -> Result<HealthResponse> {
        self.get("api/v1/health").await
    }

    pub async fn services(&self) -> Result<Vec<ServiceWithStatus>> {
        self.get("api/v1/services").await
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = self
            .base
            .join(path)
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            // Try to decode the uniform error envelope; fall back to raw text.
            let message = match resp.text().await {
                Ok(body) => serde_json::from_str::<ApiErrorBody>(&body)
                    .map(|b| b.message)
                    .unwrap_or(body),
                Err(_) => String::from("<no body>"),
            };
            return Err(ClientError::Api {
                status: status.as_u16(),
                message,
            });
        }

        resp.json::<T>()
            .await
            .map_err(|e| ClientError::Decode(e.to_string()))
    }
}
