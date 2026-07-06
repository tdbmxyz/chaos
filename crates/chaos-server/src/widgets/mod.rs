//! Dashboard widget system: layout resolution from configuration and data
//! providers with per-kind server-side caching.
//!
//! The layout (`DashboardLayout`) is resolved once at startup; each data
//! widget gets a stable instance id (`w<column>-<index>`) under which clients
//! fetch its payload from `GET /api/v1/widgets/{id}`. Upstream calls
//! (Open-Meteo, feeds, GitHub) are cached here so many open dashboards cost
//! one upstream request per TTL, and a stale payload is served when a
//! refresh fails.

mod feed;
mod posts;
mod releases;
mod stats;
mod systemd;
mod weather;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use chaos_domain::{
    ColumnSize, DashboardColumn, DashboardLayout, Widget, WidgetData, WidgetInstance,
};
use tokio::sync::RwLock;

use crate::config::{ColumnConfig, Config};

/// Resolved widget instances plus their payload caches. One per process,
/// shared via `AppState`.
pub struct WidgetHub {
    pub layout: DashboardLayout,
    /// Data widgets by instance id (static widgets carry no data).
    entries: HashMap<String, Widget>,
    cache: RwLock<HashMap<String, CacheEntry>>,
    /// Geocoded weather locations, cached for the process lifetime.
    geocode: RwLock<HashMap<String, weather::Place>>,
    /// CPU/memory sparkline samples; the sampler task only runs when the
    /// layout actually has a server_stats widget.
    stats_history: Option<stats::History>,
    http: reqwest::Client,
}

struct CacheEntry {
    fetched: Instant,
    data: WidgetData,
}

#[derive(Debug)]
pub enum WidgetError {
    UnknownWidget,
    /// The request is well-formed but not allowed by the widget's config
    /// (e.g. controlling a unit that is not on the allowlist).
    Rejected(String),
    Upstream(String),
}

impl WidgetHub {
    pub fn new(config: &Config) -> Self {
        let (layout, entries) = resolve_layout(config);
        let stats_history = entries
            .values()
            .any(|w| matches!(w, Widget::ServerStats { .. }))
            .then(stats::spawn_sampler);
        Self {
            layout,
            entries,
            cache: RwLock::new(HashMap::new()),
            geocode: RwLock::new(HashMap::new()),
            stats_history,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .user_agent(concat!("chaos/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("building widget http client"),
        }
    }

    /// Payload for one widget instance, served from cache within the TTL.
    /// On upstream failure a stale cached payload is preferred over an error.
    ///
    /// `location` is a device preference: weather widgets fetch that place
    /// instead of the configured one (each place cached separately); other
    /// widget kinds ignore it.
    pub async fn data(&self, id: &str, location: Option<&str>) -> Result<WidgetData, WidgetError> {
        let widget = self.entries.get(id).ok_or(WidgetError::UnknownWidget)?;
        let ttl = ttl(widget);

        let location = location
            .map(str::trim)
            .filter(|l| !l.is_empty() && l.len() <= 64 && matches!(widget, Widget::Weather { .. }));
        let cache_key = match location {
            Some(place) => format!("{id}@{place}"),
            None => id.to_string(),
        };

        if let Some(entry) = self.cache.read().await.get(&cache_key)
            && entry.fetched.elapsed() < ttl
        {
            return Ok(entry.data.clone());
        }

        match self.fetch(widget, location).await {
            Ok(data) => {
                self.cache.write().await.insert(
                    cache_key,
                    CacheEntry {
                        fetched: Instant::now(),
                        data: data.clone(),
                    },
                );
                Ok(data)
            }
            Err(reason) => {
                if let Some(entry) = self.cache.read().await.get(&cache_key) {
                    tracing::warn!(id, reason, "widget refresh failed, serving stale data");
                    return Ok(entry.data.clone());
                }
                tracing::warn!(id, reason, "widget fetch failed");
                Err(WidgetError::Upstream(reason))
            }
        }
    }

    async fn fetch(&self, widget: &Widget, location: Option<&str>) -> Result<WidgetData, String> {
        match widget {
            Widget::Weather {
                location: configured,
            } => weather::fetch(&self.http, &self.geocode, location.unwrap_or(configured)).await,
            Widget::Feed { urls, limit, .. } => feed::fetch(&self.http, urls, *limit).await,
            Widget::HackerNews { limit, .. } => posts::hacker_news(&self.http, *limit).await,
            Widget::Lobsters { limit, .. } => posts::lobsters(&self.http, *limit).await,
            Widget::Releases { repos, limit } => releases::fetch(&self.http, repos, *limit).await,
            Widget::ServerStats { mounts } => {
                let history = self
                    .stats_history
                    .as_ref()
                    .map(|h| {
                        h.lock()
                            .expect("stats history lock")
                            .iter()
                            .copied()
                            .collect()
                    })
                    .unwrap_or_default();
                stats::collect(mounts.clone(), history).await
            }
            Widget::Systemd { units, .. } => systemd::fetch(units).await,
            // Static widgets are never registered in `entries`.
            Widget::Search { .. }
            | Widget::Services
            | Widget::Bookmarks { .. }
            | Widget::Calendar => Err("widget has no data endpoint".into()),
        }
    }

    /// Start/stop/restart one systemd unit of a systemd widget, then return
    /// (and cache) the refreshed unit states. The unit must be configured on
    /// that widget instance and marked controllable.
    pub async fn systemd_action(
        &self,
        id: &str,
        req: &chaos_domain::SystemdActionRequest,
    ) -> Result<WidgetData, WidgetError> {
        let widget = self.entries.get(id).ok_or(WidgetError::UnknownWidget)?;
        let Widget::Systemd { units, .. } = widget else {
            return Err(WidgetError::Rejected("not a systemd widget".into()));
        };
        let def = units
            .iter()
            .find(|u| u.unit == req.unit)
            .ok_or_else(|| WidgetError::Rejected(format!("unit {:?} not configured", req.unit)))?;
        if !def.controllable {
            return Err(WidgetError::Rejected(format!(
                "unit {:?} is not controllable",
                req.unit
            )));
        }

        tracing::info!(unit = def.unit, verb = req.action.verb(), "systemd action");
        systemd::act(&def.unit, req.action)
            .await
            .map_err(WidgetError::Upstream)?;

        let data = self
            .fetch(widget, None)
            .await
            .map_err(WidgetError::Upstream)?;
        self.cache.write().await.insert(
            id.to_string(),
            CacheEntry {
                fetched: Instant::now(),
                data: data.clone(),
            },
        );
        Ok(data)
    }
}

/// How long a cached payload stays fresh, per widget kind.
fn ttl(widget: &Widget) -> Duration {
    match widget {
        Widget::Weather { .. } => Duration::from_secs(600),
        Widget::Feed { .. } => Duration::from_secs(300),
        Widget::HackerNews { .. } | Widget::Lobsters { .. } => Duration::from_secs(300),
        Widget::Releases { .. } => Duration::from_secs(1800),
        Widget::ServerStats { .. } => Duration::from_secs(10),
        Widget::Systemd { .. } => Duration::from_secs(5),
        Widget::Search { .. } | Widget::Services | Widget::Bookmarks { .. } | Widget::Calendar => {
            Duration::ZERO
        }
    }
}

/// Turn the configured columns into the wire layout, assigning instance ids
/// and filling widget fallbacks (top-level `search_url` / `bookmarks`).
/// Without configured columns, the pre-layout single-column dashboard is
/// synthesized so existing configs keep working unchanged.
fn resolve_layout(config: &Config) -> (DashboardLayout, HashMap<String, Widget>) {
    let column_defs: Vec<ColumnConfig> = if config.columns.is_empty() {
        let mut widgets = Vec::new();
        if config.search_url.is_some() {
            widgets.push(Widget::Search { search_url: None });
        }
        widgets.push(Widget::Services);
        if !config.bookmarks.is_empty() {
            widgets.push(Widget::Bookmarks { groups: Vec::new() });
        }
        vec![ColumnConfig {
            size: ColumnSize::Full,
            widgets,
        }]
    } else {
        config.columns.clone()
    };

    let mut entries = HashMap::new();
    let columns = column_defs
        .iter()
        .enumerate()
        .map(|(col_idx, col)| DashboardColumn {
            size: col.size,
            widgets: col
                .widgets
                .iter()
                .enumerate()
                .map(|(widget_idx, def)| {
                    let mut widget = def.clone();
                    match &mut widget {
                        Widget::Search { search_url } if search_url.is_none() => {
                            *search_url = config.search_url.clone();
                        }
                        Widget::Bookmarks { groups } if groups.is_empty() => {
                            *groups = config.bookmarks.clone();
                        }
                        _ => {}
                    }
                    let id = format!("w{col_idx}-{widget_idx}");
                    if widget.has_data() {
                        entries.insert(id.clone(), widget.clone());
                    }
                    WidgetInstance { id, widget }
                })
                .collect(),
        })
        .collect();

    (DashboardLayout { columns }, entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> Config {
        Config {
            search_url: Some("https://example.com/?q={}".into()),
            bookmarks: vec![chaos_domain::BookmarkGroup {
                title: "Main".into(),
                links: Vec::new(),
            }],
            ..Config::default()
        }
    }

    #[test]
    fn default_layout_mirrors_legacy_config() {
        let (layout, entries) = resolve_layout(&base_config());
        assert!(entries.is_empty());
        assert_eq!(layout.columns.len(), 1);
        let widgets: Vec<_> = layout.columns[0]
            .widgets
            .iter()
            .map(|w| w.widget.clone())
            .collect();
        assert!(matches!(&widgets[0], Widget::Search { search_url: Some(u) } if u.contains("{}")));
        assert!(matches!(widgets[1], Widget::Services));
        assert!(matches!(&widgets[2], Widget::Bookmarks { groups } if groups.len() == 1));
    }

    #[test]
    fn configured_columns_get_stable_ids_and_fallbacks() {
        let mut config = base_config();
        config.columns = vec![
            ColumnConfig {
                size: ColumnSize::Full,
                widgets: vec![Widget::Search { search_url: None }, Widget::Services],
            },
            ColumnConfig {
                size: ColumnSize::Small,
                widgets: vec![
                    Widget::Weather {
                        location: "Paris".into(),
                    },
                    Widget::ServerStats { mounts: Vec::new() },
                ],
            },
        ];

        let (layout, entries) = resolve_layout(&config);
        assert_eq!(layout.columns[1].widgets[0].id, "w1-0");
        assert_eq!(entries.len(), 2);
        assert!(matches!(
            entries.get("w1-1"),
            Some(Widget::ServerStats { .. })
        ));
        // The search fallback picked up the top-level template.
        assert!(matches!(
            &layout.columns[0].widgets[0].widget,
            Widget::Search {
                search_url: Some(_)
            }
        ));
    }

    #[tokio::test]
    async fn systemd_action_enforces_the_allowlist() {
        use chaos_domain::{SystemdAction, SystemdActionRequest, SystemdUnitDef};

        let mut config = base_config();
        config.columns = vec![ColumnConfig {
            size: ColumnSize::Full,
            widgets: vec![
                Widget::Services,
                Widget::Systemd {
                    title: None,
                    units: vec![SystemdUnitDef {
                        unit: "locked.service".into(),
                        title: None,
                        controllable: false,
                    }],
                },
            ],
        }];
        let hub = WidgetHub::new(&config);

        let req = |unit: &str| SystemdActionRequest {
            unit: unit.into(),
            action: SystemdAction::Restart,
        };
        // Unknown widget id, widget without units, unit not configured, and
        // a non-controllable unit must all be refused before any systemctl
        // call happens.
        assert!(matches!(
            hub.systemd_action("nope", &req("locked.service")).await,
            Err(WidgetError::UnknownWidget)
        ));
        assert!(matches!(
            hub.systemd_action("w0-1", &req("other.service")).await,
            Err(WidgetError::Rejected(_))
        ));
        assert!(matches!(
            hub.systemd_action("w0-1", &req("locked.service")).await,
            Err(WidgetError::Rejected(_))
        ));
    }

    #[test]
    fn columns_parse_from_toml_config() {
        use figment::providers::Format;
        let toml = r#"
            [[columns]]
            size = "small"

            [[columns.widgets]]
            type = "weather"
            location = "Paris"

            [[columns]]

            [[columns.widgets]]
            type = "services"

            [[columns.widgets]]
            type = "feed"
            title = "News"
            urls = ["https://hnrss.org/frontpage"]
            limit = 5
        "#;
        let config: Config =
            figment::Figment::from(figment::providers::Serialized::defaults(Config::default()))
                .merge(figment::providers::Toml::string(toml))
                .extract()
                .expect("config with columns parses");

        assert_eq!(config.columns.len(), 2);
        assert_eq!(config.columns[0].size, ColumnSize::Small);
        assert!(matches!(
            &config.columns[1].widgets[1],
            Widget::Feed { limit: 5, urls, .. } if urls.len() == 1
        ));
    }
}
