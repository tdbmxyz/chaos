use std::collections::HashMap;
use std::sync::Arc;

use chaos_domain::ServiceStatus;
use tokio::sync::RwLock;

use crate::config::Config;

/// Shared application state, cheap to clone (all `Arc`s inside).
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    /// Last known status per service id, written by the monitor task.
    pub statuses: Arc<RwLock<HashMap<String, ServiceStatus>>>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        Self {
            config: Arc::new(config),
            statuses: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}
