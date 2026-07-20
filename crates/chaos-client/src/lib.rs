//! Typed client for the chaos HTTP API.
//!
//! Compiles on both native (tokio + rustls) and wasm (browser fetch) targets
//! thanks to reqwest's dual backend. All UI crates go through this client so
//! the API surface is exercised from exactly one place.

mod http;
pub mod open_meteo;
pub mod posts;

use std::time::Duration;

/// Deadline for regular data requests. Generous for a LAN server; short
/// enough that an unreachable host fails the page fast instead of hanging.
const DATA_TIMEOUT: Duration = Duration::from_secs(8);
/// The health probe decides connectivity; it must answer (or fail) fast.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(3);

use chaos_domain::{
    ApiErrorBody, Calendar, CalendarEvent, CalendarRequest, Collection, CollectionRequest,
    CreateLinkRequest, DashboardLayout, Event, EventQuery, EventRequest, HealthResponse,
    HomeSensorInfo, LightCommand, LightState, Link, LinkPage, LinkQuery, LoginRequest,
    LoginResponse, SearchResults, ServiceActionRequest, ServiceWithStatus, SystemdAction,
    SystemdActionRequest, TagWithCount, TemperatureQuery, TemperatureSeries, UpdateLinkRequest,
    User, WidgetData,
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
        let req = self
            .http
            .get(self.url("api/v1/health")?)
            .timeout(HEALTH_TIMEOUT);
        self.send(req).await
    }

    pub async fn services(&self) -> Result<Vec<ServiceWithStatus>> {
        self.get("api/v1/services").await
    }

    pub async fn dashboard(&self) -> Result<DashboardLayout> {
        self.get("api/v1/dashboard").await
    }

    /// Global quick-search across services, bookmarks, links and — when
    /// signed in — calendar events. Empty/whitespace queries return the
    /// empty result set.
    pub async fn search(&self, q: &str) -> Result<SearchResults> {
        let req = self.http.get(self.url("api/v1/search")?).query(&[("q", q)]);
        self.send(req).await
    }

    /// Live payload of a data widget from the layout (feeds, stats…).
    pub async fn widget_data(&self, id: &str) -> Result<WidgetData> {
        self.get(&format!("api/v1/widgets/{id}")).await
    }

    /// Posts (HN/lobsters) for the standalone `/news` page, independent of
    /// any configured widget. Returns `WidgetData::Posts`.
    pub async fn posts_list(&self, source: chaos_domain::Source) -> Result<WidgetData> {
        self.get(&format!("api/v1/posts/{}", source.as_str())).await
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

    // ---- home ----

    /// Configured temperature sensors (empty when the Home tab is off).
    pub async fn home_sensors(&self) -> Result<Vec<HomeSensorInfo>> {
        self.get("api/v1/home/sensors").await
    }

    /// Temperature history for every configured sensor in the given range.
    pub async fn home_temperature(
        &self,
        query: &TemperatureQuery,
    ) -> Result<Vec<TemperatureSeries>> {
        let req = self
            .http
            .get(self.url("api/v1/home/temperature")?)
            .query(query);
        self.send(req).await
    }

    /// Current state of every configured light.
    pub async fn home_lights(&self) -> Result<Vec<LightState>> {
        self.get("api/v1/home/lights").await
    }

    /// Apply a partial update (on/off, brightness, color) to a light.
    pub async fn set_light(&self, id: &str, cmd: &LightCommand) -> Result<LightState> {
        let req = self
            .http
            .post(self.url(&format!("api/v1/home/lights/{id}"))?)
            .json(cmd);
        self.send(req).await
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
            .map_err(|e| ClientError::Decode(error_chain(e)))
    }

    async fn send_no_content(&self, req: reqwest::RequestBuilder) -> Result<()> {
        self.check_status(req).await.map(|_| ())
    }

    async fn check_status(&self, req: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let req = match &self.token {
            Some(token) => req.bearer_auth(token),
            None => req,
        };
        let mut request = req
            .build()
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        // Every request gets a deadline; an unreachable server must fail fast,
        // not hang "Loading" for minutes. Callers that set their own (health)
        // keep it.
        if request.timeout().is_none() {
            *request.timeout_mut() = Some(DATA_TIMEOUT);
        }
        let resp = self
            .http
            .execute(request)
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

/// Flatten an error and its source chain into one line ("outer: inner:
/// innermost"), because ClientError is stringly-typed and `Display` on
/// reqwest errors hides the serde detail that actually names the problem.
fn error_chain(e: impl std::error::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(inner) = source {
        out.push_str(": ");
        out.push_str(&inner.to_string());
        source = inner.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fmt;

    #[derive(Debug)]
    struct Outer(Inner);
    #[derive(Debug)]
    struct Inner;
    impl fmt::Display for Outer {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "error decoding response body")
        }
    }
    impl fmt::Display for Inner {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "missing field `on` at line 1 column 12")
        }
    }
    impl std::error::Error for Inner {}
    impl std::error::Error for Outer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn error_chain_includes_sources() {
        assert_eq!(
            super::error_chain(Outer(Inner)),
            "error decoding response body: missing field `on` at line 1 column 12"
        );
    }
}
