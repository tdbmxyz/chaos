use std::collections::HashMap;
use std::sync::Arc;

use chaos_domain::ServiceStatus;
use tokio::sync::{Notify, RwLock};

use crate::config::Config;
use crate::db::Db;
use crate::home_assistant::HomeAssistantClient;
use crate::ics::FeedCache;
use crate::widgets::WidgetHub;

/// Shared application state, cheap to clone (all `Arc`s / pools inside).
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: Db,
    /// Outbound client for page metadata fetches.
    pub fetcher: reqwest::Client,
    /// Wakes the archiver when a link becomes pending.
    pub archive_notify: Arc<Notify>,
    /// Last known status per service id, written by the monitor task.
    pub statuses: Arc<RwLock<HashMap<String, ServiceStatus>>>,
    /// Resolved dashboard layout and widget data caches.
    pub widgets: Arc<WidgetHub>,
    /// Parsed ICS calendar feeds, cached per calendar id.
    pub ics: Arc<FeedCache>,
    /// Home Assistant client, when the Home tab is configured.
    pub home: Option<Arc<HomeAssistantClient>>,
    /// Failed-login backoff tracker (in-memory, per username+IP).
    pub login_throttle: Arc<crate::auth::LoginThrottle>,
}

impl AppState {
    pub fn new(config: Config, db: Db) -> anyhow::Result<Self> {
        let widgets = Arc::new(WidgetHub::new(&config));
        let home = HomeAssistantClient::new(&config.home_assistant)?.map(Arc::new);
        Ok(Self {
            config: Arc::new(config),
            db,
            fetcher: crate::metadata::http_client(),
            archive_notify: Arc::new(Notify::new()),
            statuses: Arc::new(RwLock::new(HashMap::new())),
            widgets,
            ics: Arc::new(FeedCache::default()),
            home,
            login_throttle: Arc::new(crate::auth::LoginThrottle::default()),
        })
    }
}
