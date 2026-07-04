use std::collections::HashMap;
use std::sync::Arc;

use chaos_domain::ServiceStatus;
use tokio::sync::RwLock;

use crate::config::Config;
use crate::db::Db;

/// Shared application state, cheap to clone (all `Arc`s / pools inside).
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: Db,
    /// Last known status per service id, written by the monitor task.
    pub statuses: Arc<RwLock<HashMap<String, ServiceStatus>>>,
}

impl AppState {
    pub fn new(config: Config, db: Db) -> Self {
        Self {
            config: Arc::new(config),
            db,
            statuses: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}
