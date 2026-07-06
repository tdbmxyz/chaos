//! Dashboard layout and widgets (glance-style), defined in server
//! configuration and served read-only to the clients.
//!
//! Widgets split in two: [`Widget`] is the *definition* (what to show, part
//! of the layout), [`WidgetData`] is the *live payload* fetched separately
//! per widget instance from `/api/v1/widgets/{id}` so the server can cache
//! upstream calls (Open-Meteo, feeds, GitHub) independently of the layout.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    pub title: String,
    pub url: Url,
    /// Same icon conventions as services (`di:`, `si:`, `sh:` or URL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookmarkGroup {
    pub title: String,
    #[serde(default)]
    pub links: Vec<Bookmark>,
}

// ---- layout ----

/// A widget as declared in configuration (and echoed in the layout).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Widget {
    /// Search box; `search_url` is a template where `{}` is replaced by the
    /// url-encoded query. Falls back to the server-level `search_url`.
    Search {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        search_url: Option<String>,
    },
    /// The monitored-services grid (data comes from `/api/v1/services`).
    Services,
    /// Static bookmark groups; falls back to the server-level `bookmarks`.
    Bookmarks {
        #[serde(default)]
        groups: Vec<BookmarkGroup>,
    },
    /// Current weather + short forecast via Open-Meteo.
    Weather {
        /// Place name, geocoded server-side (e.g. `"Paris"` or `"Lyon, FR"`).
        location: String,
    },
    /// Merged RSS/Atom feed list (covers HN and lobsters via their feeds).
    Feed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        urls: Vec<Url>,
        #[serde(default = "default_feed_limit")]
        limit: u32,
    },
    /// Latest GitHub release per repository (`owner/name`).
    Releases {
        repos: Vec<String>,
        #[serde(default = "default_releases_limit")]
        limit: u32,
    },
    /// Metrics of the host running chaos-server.
    ServerStats {
        /// Only show these mount points; empty shows every real filesystem
        /// (which gets noisy with zfs/btrfs datasets).
        #[serde(default)]
        mounts: Vec<String>,
    },
    /// Static month view, rendered entirely client-side.
    Calendar,
    /// State (and optional control) of systemd units on the server host.
    /// Only the units listed here can ever be touched; control actions go
    /// through `POST /api/v1/widgets/{id}/systemd`.
    Systemd {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        units: Vec<SystemdUnitDef>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemdUnitDef {
    /// Full unit name, e.g. `"stirling-pdf.service"`.
    pub unit: String,
    /// Display name; defaults to the unit name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Whether start/stop/restart buttons are offered (status is always
    /// shown). Defaults to true — listing a unit is already an opt-in.
    #[serde(default = "default_true")]
    pub controllable: bool,
}

fn default_true() -> bool {
    true
}

fn default_feed_limit() -> u32 {
    15
}

fn default_releases_limit() -> u32 {
    10
}

impl Widget {
    /// Whether this widget has a server-side data payload (`WidgetData`).
    pub fn has_data(&self) -> bool {
        matches!(
            self,
            Widget::Weather { .. }
                | Widget::Feed { .. }
                | Widget::Releases { .. }
                | Widget::ServerStats { .. }
                | Widget::Systemd { .. }
        )
    }
}

/// A widget placed in the layout, with the server-assigned instance id used
/// to fetch its data from `/api/v1/widgets/{id}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WidgetInstance {
    pub id: String,
    #[serde(flatten)]
    pub widget: Widget,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnSize {
    /// Regular column, shares the remaining width evenly.
    #[default]
    Full,
    /// Narrow fixed-width side column.
    Small,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DashboardColumn {
    pub size: ColumnSize,
    pub widgets: Vec<WidgetInstance>,
}

/// What `/api/v1/dashboard` returns: the fully resolved layout.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DashboardLayout {
    pub columns: Vec<DashboardColumn>,
}

// ---- widget data ----

/// Live payload of a data widget, tagged like [`Widget`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WidgetData {
    Weather(WeatherData),
    Feed { items: Vec<FeedItem> },
    Releases { items: Vec<ReleaseItem> },
    ServerStats(ServerStats),
    Systemd { units: Vec<SystemdUnitStatus> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemdUnitStatus {
    pub unit: String,
    pub title: String,
    /// systemd ActiveState (`active`, `inactive`, `failed`, `activating`,
    /// `deactivating`, `reloading`) or `not-found` for unknown units.
    pub active_state: String,
    pub sub_state: String,
    pub controllable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeatherData {
    /// Resolved place name (from geocoding, nicer than the config string).
    pub location: String,
    pub temperature_c: f64,
    pub apparent_c: f64,
    pub humidity_pct: Option<f64>,
    pub wind_kmh: f64,
    /// WMO weather interpretation code.
    pub weather_code: i32,
    /// Human description of `weather_code`.
    pub description: String,
    pub daily: Vec<DailyForecast>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DailyForecast {
    pub date: NaiveDate,
    pub min_c: f64,
    pub max_c: f64,
    pub weather_code: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedItem {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<Url>,
    /// Title of the feed the item came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseItem {
    /// `owner/name`.
    pub repo: String,
    pub tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<Url>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerStats {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    pub uptime_secs: u64,
    /// 1, 5 and 15 minute load averages.
    pub load_avg: [f64; 3],
    pub mem_total_bytes: u64,
    pub mem_used_bytes: u64,
    pub disks: Vec<DiskUsage>,
    /// Oldest-first samples taken every [`ServerStats::HISTORY_INTERVAL_SECS`]
    /// seconds, capped at [`ServerStats::HISTORY_LEN`] (≈ 1 hour).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<StatPoint>,
}

impl ServerStats {
    pub const HISTORY_INTERVAL_SECS: u64 = 30;
    pub const HISTORY_LEN: usize = 120;
}

/// One sample of the stats history sparklines.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StatPoint {
    /// Overall CPU utilisation since the previous sample, 0–100.
    pub cpu_pct: f32,
    pub mem_used_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskUsage {
    pub mount: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
}
