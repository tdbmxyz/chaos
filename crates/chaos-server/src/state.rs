use std::collections::HashMap;
use std::sync::Arc;

use chaos_domain::ServiceStatus;
use tokio::sync::{Notify, RwLock};

use crate::config::Config;
use crate::db::Db;
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
}

impl AppState {
    pub fn new(config: Config, db: Db) -> Self {
        let widgets = Arc::new(WidgetHub::new(&config));
        Self {
            config: Arc::new(config),
            db,
            fetcher: crate::metadata::http_client(),
            archive_notify: Arc::new(Notify::new()),
            statuses: Arc::new(RwLock::new(HashMap::new())),
            widgets,
        }
    }
}
