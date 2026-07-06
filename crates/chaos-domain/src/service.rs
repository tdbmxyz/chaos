//! Monitored services shown on the dashboard (the glance "monitor" widget).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;

/// A service registered on the dashboard. On NixOS this list is generated
/// from `config.modules.server.servicesList`, mirroring what glance.nix does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceDef {
    /// Stable identifier (slug), e.g. `"jellyfin"`.
    pub id: String,
    /// Human title shown on the dashboard, e.g. `"Jellyfin"`.
    pub title: String,
    /// URL the user is sent to when clicking the tile.
    pub url: Url,
    /// Icon reference. Same conventions as glance: `di:jellyfin` (dashboard
    /// icons), `si:github` (simple icons), or an absolute URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// URL polled by the health monitor. Defaults to `url` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_url: Option<Url>,
    /// Systemd unit backing an *on-demand* service (heavy, kept stopped at
    /// rest). When set, the monitor asks systemd before probing HTTP — an
    /// inactive unit shows as [`HealthState::Paused`] instead of down — and
    /// the tile offers start/stop through
    /// `POST /api/v1/services/{id}/systemd` (the unit must also be listed
    /// in the polkit allowlist, `services.chaos.systemdControl.units`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

impl ServiceDef {
    /// The URL the monitor should poll for this service.
    pub fn effective_check_url(&self) -> &Url {
        self.check_url.as_ref().unwrap_or(&self.url)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    /// Service answered (any HTTP response below 500 counts: a 401 from an
    /// authenticated UI still proves the service is alive).
    Up,
    /// Service answered with a server error (5xx).
    Degraded,
    /// Connection failed or timed out.
    Down,
    /// On-demand service whose unit is deliberately stopped (its default
    /// state); no HTTP check is made.
    Paused,
    /// On-demand service whose unit is running but not answering HTTP yet
    /// (units report active before slow apps bind their port).
    Starting,
    /// Not checked yet.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub state: HealthState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<DateTime<Utc>>,
    /// Human-readable cause when `state` is `Down`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ServiceStatus {
    pub fn unknown() -> Self {
        Self {
            state: HealthState::Unknown,
            http_status: None,
            latency_ms: None,
            checked_at: None,
            error: None,
        }
    }
}

/// What the dashboard actually renders: definition + last known status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceWithStatus {
    #[serde(flatten)]
    pub def: ServiceDef,
    pub status: ServiceStatus,
}

/// A companion application integrated into chaos ("plugin", e.g. yomu).
/// Configured server-side; clients only see apps the admin activated, so
/// nothing shows up otherwise. Web/desktop embed the app's UI; the Android
/// shell launches the native app when `android_package` is installed and
/// falls back to the URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppLink {
    /// Stable identifier (slug), e.g. `"yomu"`.
    pub id: String,
    /// Sidebar label, e.g. `"Yomu"`.
    pub title: String,
    /// Where the app's web UI lives, e.g. `"http://zeus:4700"`.
    pub url: Url,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Android application id to launch instead of the URL, e.g.
    /// `"xyz.tdbm.yomu"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub android_package: Option<String>,
}
