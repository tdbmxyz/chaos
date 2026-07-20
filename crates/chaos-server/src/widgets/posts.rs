//! Link-aggregator providers: Hacker News and Lobsters, with points and
//! comment counts (their RSS feeds carry neither, hence not `feed.rs`).
//!
//! Both produce [`WidgetData::Posts`]: the top-by-upvotes links of the
//! trailing 24 h, 48 h and week. Hacker News comes from the Algolia archive
//! API (one `front_page` query per window, re-sorted by points — Algolia
//! ranks by relevance). Lobsters pages `newest.json` back until the sweep
//! covers a week, then buckets by `created_at`.

use chaos_domain::{FeedItem, PostsData, WidgetData};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use url::Url;

use crate::http_util::get_json;

const HN_ITEM: &str = "https://news.ycombinator.com/item?id=";
const ALGOLIA_SEARCH: &str = "https://hn.algolia.com/api/v1/search";
const LOBSTERS_NEWEST: &str = "https://lobste.rs/newest.json";
/// newest.json covers ~1.3 days per 25-story page; 10 pages safely spans a week.
const LOBSTERS_PAGE_CAP: u32 = 10;
const WEEK_HOURS: i64 = 24 * 7;

#[derive(Deserialize)]
struct AlgoliaResponse {
    hits: Vec<AlgoliaHit>,
}

#[derive(Deserialize)]
struct AlgoliaHit {
    title: Option<String>,
    url: Option<String>,
    points: Option<u64>,
    num_comments: Option<u64>,
    created_at_i: Option<i64>,
    #[serde(rename = "objectID")]
    object_id: String,
}

#[derive(Deserialize)]
struct LobstersStory {
    short_id: String,
    title: String,
    url: String,
    score: i64,
    comment_count: u64,
    comments_url: Url,
    created_at: Option<DateTime<Utc>>,
}

/// Once gathered, both aggregators are shown by upvotes, not by their
/// route's own ranking (Algolia relevance / newest-first pages). Stable,
/// so equal scores keep the upstream order.
fn sort_by_score(items: &mut [FeedItem]) {
    items.sort_by_key(|item| std::cmp::Reverse(item.score));
}

fn algolia_item(hit: AlgoliaHit) -> FeedItem {
    let discussion: Option<Url> = format!("{HN_ITEM}{}", hit.object_id).parse().ok();
    FeedItem {
        title: hit.title.unwrap_or_else(|| "(untitled)".into()),
        // Ask HN / Show HN text posts have no external URL.
        url: hit
            .url
            .filter(|u| !u.is_empty())
            .and_then(|u| u.parse().ok())
            .or_else(|| discussion.clone()),
        source: Some("Hacker News".into()),
        published: hit
            .created_at_i
            .and_then(|t| DateTime::from_timestamp(t, 0)),
        score: hit.points,
        comments: hit.num_comments,
        comments_url: discussion,
        id: Some(hit.object_id.clone()),
    }
}

/// Items published within the trailing `hours`, by upvotes, top `limit`.
fn windowed(items: &[FeedItem], now: DateTime<Utc>, hours: i64, limit: u32) -> Vec<FeedItem> {
    let cutoff = now - chrono::Duration::hours(hours);
    let mut hits: Vec<FeedItem> = items
        .iter()
        .filter(|i| i.published.is_some_and(|p| p >= cutoff))
        .cloned()
        .collect();
    sort_by_score(&mut hits);
    hits.truncate(limit as usize);
    hits
}

pub async fn hacker_news(
    http: &reqwest::Client,
    limit: u32,
    now: DateTime<Utc>,
) -> Result<WidgetData, String> {
    // One archive query per window: relevance-ranked top-50 among stories
    // that made the front page in the window; re-sorted by points below.
    let window = |hours: i64| async move {
        let cutoff = (now - chrono::Duration::hours(hours)).timestamp();
        let url = format!(
            "{ALGOLIA_SEARCH}?tags=front_page&numericFilters=created_at_i>{cutoff}&hitsPerPage=50"
        );
        let resp: AlgoliaResponse = get_json(http, &url)
            .await
            .map_err(|e| format!("hn algolia: {e}"))?;
        let items: Vec<FeedItem> = resp.hits.into_iter().map(algolia_item).collect();
        Ok::<_, String>(windowed(&items, now, hours, limit))
    };
    let (last_24h, last_48h, last_week) =
        futures::try_join!(window(24), window(48), window(WEEK_HOURS))?;
    if last_week.is_empty() {
        return Err("no stories returned".into());
    }
    Ok(WidgetData::Posts(PostsData {
        last_24h,
        last_48h,
        last_week,
    }))
}

pub async fn lobsters(
    http: &reqwest::Client,
    limit: u32,
    now: DateTime<Utc>,
) -> Result<WidgetData, String> {
    let cutoff = now - chrono::Duration::hours(WEEK_HOURS);
    let mut items: Vec<FeedItem> = Vec::new();
    for page in 1..=LOBSTERS_PAGE_CAP {
        let stories: Vec<LobstersStory> = get_json(http, &format!("{LOBSTERS_NEWEST}?page={page}"))
            .await
            .map_err(|e| format!("lobsters: {e}"))?;
        if stories.is_empty() {
            break;
        }
        // Newest-first: once a page's oldest story predates the cutoff,
        // later pages are all older.
        let done = stories
            .last()
            .and_then(|s| s.created_at)
            .is_none_or(|t| t < cutoff);
        items.extend(stories.into_iter().map(lobsters_item));
        if done {
            break;
        }
    }
    if items.is_empty() {
        return Err("no stories returned".into());
    }
    Ok(WidgetData::Posts(PostsData {
        last_24h: windowed(&items, now, 24, limit),
        last_48h: windowed(&items, now, 48, limit),
        last_week: windowed(&items, now, WEEK_HOURS, limit),
    }))
}

fn lobsters_item(story: LobstersStory) -> FeedItem {
    let discussion = story.comments_url;
    let short_id = story.short_id;
    FeedItem {
        title: story.title,
        // Text posts have an empty url; point at the discussion instead.
        url: Some(story.url)
            .filter(|u| !u.is_empty())
            .and_then(|u| u.parse().ok())
            .or_else(|| Some(discussion.clone())),
        source: Some("Lobsters".into()),
        published: story.created_at,
        score: u64::try_from(story.score).ok(),
        comments: Some(story.comment_count),
        comments_url: Some(discussion),
        id: Some(short_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank_item() -> FeedItem {
        FeedItem {
            title: String::new(),
            url: None,
            source: None,
            published: None,
            score: None,
            comments: None,
            comments_url: None,
            id: None,
        }
    }

    #[test]
    fn feeds_are_ordered_by_score_descending_scoreless_last() {
        let mut items = vec![
            FeedItem {
                title: "low".into(),
                score: Some(3),
                ..blank_item()
            },
            FeedItem {
                title: "none".into(),
                score: None,
                ..blank_item()
            },
            FeedItem {
                title: "high".into(),
                score: Some(90),
                ..blank_item()
            },
        ];
        sort_by_score(&mut items);
        let titles: Vec<_> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["high", "low", "none"]);
    }

    #[test]
    fn algolia_hit_maps_points_comments_and_discussion() {
        let raw = r#"{"title":"Ask HN: editors?","url":null,"points":73,"num_comments":41,"created_at_i":1783300000,"objectID":"9876","author":"pg","_tags":["story","front_page"]}"#;
        let item = algolia_item(serde_json::from_str(raw).expect("parse"));
        assert_eq!(item.title, "Ask HN: editors?");
        assert_eq!(item.score, Some(73));
        assert_eq!(item.comments, Some(41));
        assert_eq!(
            item.comments_url.as_ref().map(Url::as_str),
            Some("https://news.ycombinator.com/item?id=9876")
        );
        // No external url: point at the discussion instead.
        assert_eq!(item.url, item.comments_url);
        assert!(item.published.is_some());
    }

    #[test]
    fn algolia_hit_keeps_the_external_url() {
        let raw = r#"{"title":"Rust 2.0","url":"https://blog.rust-lang.org/2","points":256,"num_comments":142,"created_at_i":1783300000,"objectID":"1234"}"#;
        let item = algolia_item(serde_json::from_str(raw).expect("parse"));
        assert_eq!(item.url.unwrap().as_str(), "https://blog.rust-lang.org/2");
        assert_eq!(item.source.as_deref(), Some("Hacker News"));
    }

    #[test]
    fn windowed_filters_sorts_and_truncates() {
        let now = Utc::now();
        let aged = |hours: i64, score: u64, title: &str| FeedItem {
            title: title.into(),
            published: Some(now - chrono::Duration::hours(hours)),
            score: Some(score),
            ..blank_item()
        };
        let items = vec![
            aged(1, 5, "fresh"),
            aged(30, 50, "yesterday"),
            aged(24 * 6, 9, "last week"),
            aged(24 * 9, 99, "too old"),
            FeedItem {
                title: "undated".into(),
                score: Some(100),
                ..blank_item()
            },
        ];
        let titles = |window: Vec<FeedItem>| -> Vec<String> {
            window.into_iter().map(|i| i.title).collect()
        };
        assert_eq!(titles(windowed(&items, now, 24, 10)), ["fresh"]);
        assert_eq!(
            titles(windowed(&items, now, 48, 10)),
            ["yesterday", "fresh"]
        );
        // The 9-day-old and unpublished items never qualify.
        assert_eq!(
            titles(windowed(&items, now, WEEK_HOURS, 10)),
            ["yesterday", "last week", "fresh"]
        );
        assert_eq!(
            titles(windowed(&items, now, WEEK_HOURS, 2)),
            ["yesterday", "last week"]
        );
    }

    #[test]
    fn maps_a_lobsters_story() {
        let raw = r#"{"short_id":"abc123","title":"A story","url":"https://example.org/post","score":31,"comment_count":7,"comments_url":"https://lobste.rs/s/abc123/a_story","created_at":"2026-07-06T01:02:03Z","tags":["rust"]}"#;
        let item = lobsters_item(serde_json::from_str(raw).expect("parse"));
        assert_eq!(item.score, Some(31));
        assert_eq!(item.comments, Some(7));
        assert_eq!(item.url.unwrap().as_str(), "https://example.org/post");
        assert_eq!(
            item.comments_url.unwrap().as_str(),
            "https://lobste.rs/s/abc123/a_story"
        );
    }

    #[test]
    fn mapped_items_carry_a_non_empty_id() {
        let raw_hn = r#"{"title":"t","url":null,"points":1,"num_comments":0,"created_at_i":1783300000,"objectID":"9876"}"#;
        let hn = algolia_item(serde_json::from_str(raw_hn).expect("parse"));
        assert_eq!(hn.id.as_deref(), Some("9876"));
        let raw_lob = r#"{"short_id":"abc123","title":"A","url":"https://e.org","score":1,"comment_count":0,"comments_url":"https://lobste.rs/s/abc123","created_at":"2026-07-06T01:02:03Z"}"#;
        let lob = lobsters_item(serde_json::from_str(raw_lob).expect("parse"));
        assert_eq!(lob.id.as_deref(), Some("abc123"));
        assert!(hn.id.is_some_and(|id| !id.is_empty()));
        assert!(lob.id.is_some_and(|id| !id.is_empty()));
    }

    #[test]
    fn lobsters_text_post_falls_back_to_discussion() {
        let raw = r#"{"short_id":"x","title":"Meta","url":"","score":5,"comment_count":2,"comments_url":"https://lobste.rs/s/x/meta","created_at":"2026-07-06T01:02:03Z"}"#;
        let item = lobsters_item(serde_json::from_str(raw).expect("parse"));
        assert_eq!(item.url.unwrap().as_str(), "https://lobste.rs/s/x/meta");
    }
}
