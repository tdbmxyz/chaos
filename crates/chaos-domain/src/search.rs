//! Global quick-search (`GET /api/v1/search`): grouped hits across
//! config-defined services and bookmarks, stored links, and the signed-in
//! user's calendar events.

use serde::{Deserialize, Serialize};
use url::Url;

/// Which group a hit belongs to; the UI routes on it (events have no URL
/// of their own and navigate to `/calendar`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchKind {
    Service,
    Bookmark,
    Link,
    Event,
}

/// One quick-search result row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    pub kind: SearchKind,
    pub title: String,
    /// Context line under the title: URL host for services/links, group
    /// name for bookmarks, start time + calendar name for events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// Where the hit leads. `None` for hits the UI routes internally
    /// (events open the calendar page).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<Url>,
}

/// What `GET /api/v1/search` returns: hits grouped in display order.
/// Groups the requester cannot see (events while logged off) are empty,
/// never an error.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchResults {
    pub services: Vec<SearchHit>,
    pub bookmarks: Vec<SearchHit>,
    pub links: Vec<SearchHit>,
    pub events: Vec<SearchHit>,
}

/// Query parameters of `GET /api/v1/search`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    pub q: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_wire_format_is_stable() {
        let hit = SearchHit {
            kind: SearchKind::Service,
            title: "Jellyfin".into(),
            subtitle: None,
            url: Some("http://zeus:8096".parse().unwrap()),
        };
        let json = serde_json::to_string(&hit).unwrap();
        // Enum tags are snake_case and empty optionals are omitted, like
        // every other wire type in this crate.
        assert!(json.contains(r#""kind":"service""#), "got {json}");
        assert!(!json.contains("subtitle"), "got {json}");
        let back: SearchHit = serde_json::from_str(&json).unwrap();
        assert_eq!(back, hit);

        // Every group is optional on the wire; `{}` is the empty result.
        let results: SearchResults = serde_json::from_str("{}").unwrap();
        assert_eq!(results, SearchResults::default());
        assert!(results.services.is_empty() && results.events.is_empty());
    }
}
