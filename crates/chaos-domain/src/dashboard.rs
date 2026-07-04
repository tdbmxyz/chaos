//! Static dashboard content (glance-style bookmarks and search), defined in
//! server configuration and served read-only to the clients.

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

/// What the dashboard needs besides live service statuses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardConfig {
    /// Search engine template; `{}` is replaced by the url-encoded query,
    /// e.g. `https://searx.example/search?q={}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_url: Option<String>,
    #[serde(default)]
    pub bookmarks: Vec<BookmarkGroup>,
}
