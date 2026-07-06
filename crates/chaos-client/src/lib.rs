//! Typed client for the chaos HTTP API.
//!
//! Compiles on both native (tokio + rustls) and wasm (browser fetch) targets
//! thanks to reqwest's dual backend. All UI crates go through this client so
//! the API surface is exercised from exactly one place.

use chaos_domain::{
    ApiErrorBody, AppLink, Calendar, CalendarEvent, CalendarRequest, Collection, CollectionRequest,
    CreateLinkRequest, DashboardLayout, Event, EventQuery, EventRequest, HealthResponse, Link,
    LinkPage, LinkQuery, LoginRequest, LoginResponse, ServiceActionRequest, ServiceWithStatus,
    SystemdAction, SystemdActionRequest, TagWithCount, UpdateLinkRequest, User, WidgetData,
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

impl ClientError {
    /// True when the server rejected the session (signed off or expired).
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, ClientError::Api { status: 401, .. })
    }
}

pub type Result<T> = std::result::Result<T, ClientError>;

#[derive(Debug, Clone)]
pub struct ChaosClient {
    base: Url,
    http: reqwest::Client,
    /// Session token sent as `Authorization: Bearer`. Browsers leave this
    /// unset and rely on the same-origin session cookie instead; native
    /// clients (desktop/mobile) store the token from `login`.
    token: Option<String>,
}

impl ChaosClient {
    /// `base` is the server origin, e.g. `http://zeus:4600` — without the
    /// `/api/v1` prefix, which the client appends itself.
    pub fn new(base: Url) -> Self {
        Self {
            base,
            http: reqwest::Client::new(),
            token: None,
        }
    }

    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token;
        self
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

    pub async fn dashboard(&self) -> Result<DashboardLayout> {
        self.get("api/v1/dashboard").await
    }

    /// Companion apps activated in the server config (empty = none).
    pub async fn apps(&self) -> Result<Vec<AppLink>> {
        self.get("api/v1/apps").await
    }

    /// Live payload of a data widget from the layout (weather, feeds…).
    /// `location` is the device's weather-location preference; ignored by
    /// every widget kind except weather.
    pub async fn widget_data(&self, id: &str, location: Option<&str>) -> Result<WidgetData> {
        let mut req = self.http.get(self.url(&format!("api/v1/widgets/{id}"))?);
        if let Some(location) = location {
            req = req.query(&[("location", location)]);
        }
        self.send(req).await
    }

    /// Start/stop an on-demand service's systemd unit (services configured
    /// with a `unit`); returns the service with its re-checked status.
    pub async fn service_action(
        &self,
        id: &str,
        action: SystemdAction,
    ) -> Result<ServiceWithStatus> {
        let req = self
            .http
            .post(self.url(&format!("api/v1/services/{id}/systemd"))?)
            .json(&ServiceActionRequest { action });
        self.send(req).await
    }

    /// Start/stop/restart a unit of a systemd widget; returns the refreshed
    /// unit states of that widget.
    pub async fn systemd_action(&self, id: &str, req: &SystemdActionRequest) -> Result<WidgetData> {
        let req = self
            .http
            .post(self.url(&format!("api/v1/widgets/{id}/systemd"))?)
            .json(req);
        self.send(req).await
    }

    /// Server-cached icon for a `di:`/`si:`/`sh:` spec; direct URLs pass
    /// through unchanged.
    pub fn icon_url(&self, spec: &str) -> Option<Url> {
        if spec.starts_with("http://") || spec.starts_with("https://") {
            return spec.parse().ok();
        }
        self.base.join(&format!("api/v1/icons/{spec}")).ok()
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

    // ---- archive ----

    /// Queue (or re-queue) a page snapshot; returns the link in pending state.
    pub async fn archive_link(&self, id: Uuid) -> Result<Link> {
        let req = self
            .http
            .post(self.url(&format!("api/v1/links/{id}/archive"))?);
        self.send(req).await
    }

    /// Where the archived copy of a link is served (for direct browser use).
    pub fn archive_view_url(&self, id: Uuid) -> Option<Url> {
        self.base.join(&format!("api/v1/links/{id}/archive")).ok()
    }

    // ---- tags ----

    pub async fn list_tags(&self) -> Result<Vec<TagWithCount>> {
        self.get("api/v1/tags").await
    }

    // ---- auth ----

    pub async fn login(&self, req: &LoginRequest) -> Result<LoginResponse> {
        let req = self.http.post(self.url("api/v1/auth/login")?).json(req);
        self.send(req).await
    }

    pub async fn logout(&self) -> Result<()> {
        let req = self.http.post(self.url("api/v1/auth/logout")?);
        self.send_no_content(req).await
    }

    /// The signed-in user, or an `Api { status: 401 }` error when logged off.
    pub async fn me(&self) -> Result<User> {
        self.get("api/v1/auth/me").await
    }

    // ---- calendars & events ----

    pub async fn list_calendars(&self) -> Result<Vec<Calendar>> {
        self.get("api/v1/calendars").await
    }

    pub async fn create_calendar(&self, req: &CalendarRequest) -> Result<Calendar> {
        let req = self.http.post(self.url("api/v1/calendars")?).json(req);
        self.send(req).await
    }

    pub async fn update_calendar(&self, id: Uuid, req: &CalendarRequest) -> Result<Calendar> {
        let req = self
            .http
            .put(self.url(&format!("api/v1/calendars/{id}"))?)
            .json(req);
        self.send(req).await
    }

    pub async fn delete_calendar(&self, id: Uuid) -> Result<()> {
        let req = self
            .http
            .delete(self.url(&format!("api/v1/calendars/{id}"))?);
        self.send_no_content(req).await
    }

    /// Drop the server's cached ICS feeds so the next query refetches.
    pub async fn refresh_calendars(&self) -> Result<()> {
        let req = self.http.post(self.url("api/v1/calendar/refresh")?);
        self.send_no_content(req).await
    }

    /// Merged events (local + feeds) across all the user's calendars.
    pub async fn calendar_events(&self, query: &EventQuery) -> Result<Vec<CalendarEvent>> {
        let req = self
            .http
            .get(self.url("api/v1/calendar/events")?)
            .query(query);
        self.send(req).await
    }

    pub async fn create_event(&self, req: &EventRequest) -> Result<Event> {
        let req = self.http.post(self.url("api/v1/events")?).json(req);
        self.send(req).await
    }

    pub async fn update_event(&self, id: Uuid, req: &EventRequest) -> Result<Event> {
        let req = self
            .http
            .put(self.url(&format!("api/v1/events/{id}"))?)
            .json(req);
        self.send(req).await
    }

    pub async fn delete_event(&self, id: Uuid) -> Result<()> {
        let req = self.http.delete(self.url(&format!("api/v1/events/{id}"))?);
        self.send_no_content(req).await
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
        let resp = self.check_status(req).await?;
        resp.json::<T>()
            .await
            .map_err(|e| ClientError::Decode(e.to_string()))
    }

    async fn send_no_content(&self, req: reqwest::RequestBuilder) -> Result<()> {
        self.check_status(req).await.map(|_| ())
    }

    async fn check_status(&self, req: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let req = match &self.token {
            Some(token) => req.bearer_auth(token),
            None => req,
        };
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
