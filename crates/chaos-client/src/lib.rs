//! Typed client for the chaos HTTP API.
//!
//! Compiles on both native (tokio + rustls) and wasm (browser fetch) targets
//! thanks to reqwest's dual backend. All UI crates go through this client so
//! the API surface is exercised from exactly one place.

use chaos_domain::{
    ApiErrorBody, Collection, CollectionRequest, CreateLinkRequest, HealthResponse, Link, LinkPage,
    LinkQuery, ServiceWithStatus, TagWithCount, UpdateLinkRequest,
};
use url::Url;
use uuid::Uuid;

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

    // ---- links ----

    pub async fn list_links(&self, query: &LinkQuery) -> Result<LinkPage> {
        let req = self.http.get(self.url("api/v1/links")?).query(query);
        self.send(req).await
    }

    pub async fn get_link(&self, id: Uuid) -> Result<Link> {
        self.get(&format!("api/v1/links/{id}")).await
    }

    pub async fn create_link(&self, req: &CreateLinkRequest) -> Result<Link> {
        let req = self.http.post(self.url("api/v1/links")?).json(req);
        self.send(req).await
    }

    pub async fn update_link(&self, id: Uuid, req: &UpdateLinkRequest) -> Result<Link> {
        let req = self
            .http
            .put(self.url(&format!("api/v1/links/{id}"))?)
            .json(req);
        self.send(req).await
    }

    pub async fn delete_link(&self, id: Uuid) -> Result<()> {
        let req = self.http.delete(self.url(&format!("api/v1/links/{id}"))?);
        self.send_no_content(req).await
    }

    // ---- collections ----

    pub async fn list_collections(&self) -> Result<Vec<Collection>> {
        self.get("api/v1/collections").await
    }

    pub async fn create_collection(&self, req: &CollectionRequest) -> Result<Collection> {
        let req = self.http.post(self.url("api/v1/collections")?).json(req);
        self.send(req).await
    }

    pub async fn update_collection(&self, id: Uuid, req: &CollectionRequest) -> Result<Collection> {
        let req = self
            .http
            .put(self.url(&format!("api/v1/collections/{id}"))?)
            .json(req);
        self.send(req).await
    }

    pub async fn delete_collection(&self, id: Uuid) -> Result<()> {
        let req = self
            .http
            .delete(self.url(&format!("api/v1/collections/{id}"))?);
        self.send_no_content(req).await
    }

    // ---- tags ----

    pub async fn list_tags(&self) -> Result<Vec<TagWithCount>> {
        self.get("api/v1/tags").await
    }

    // ---- plumbing ----

    fn url(&self, path: &str) -> Result<Url> {
        self.base
            .join(path)
            .map_err(|e| ClientError::Transport(e.to_string()))
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.send(self.http.get(self.url(path)?)).await
    }

    async fn send<T: serde::de::DeserializeOwned>(
        &self,
        req: reqwest::RequestBuilder,
    ) -> Result<T> {
        let resp = Self::check_status(req).await?;
        resp.json::<T>()
            .await
            .map_err(|e| ClientError::Decode(e.to_string()))
    }

    async fn send_no_content(&self, req: reqwest::RequestBuilder) -> Result<()> {
        Self::check_status(req).await.map(|_| ())
    }

    async fn check_status(req: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let resp = req
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
        Ok(resp)
    }
}
