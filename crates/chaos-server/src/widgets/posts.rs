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
/// Only count stories that cleared this many upvotes — a "notable story" floor
/// that replaces the sparse, bursty `front_page` tag (which left the 24-48h tab
/// empty and even missed the week's biggest stories). Stories above it are
/// plentiful and evenly spread across the week.
const HN_MIN_POINTS: u32 = 50;
/// Base for the paginated newest feed. The page number goes in the PATH
/// (`/newest/page/{n}.json`) — the `?page=` query form is silently ignored by
/// lobste.rs and returns page 1 every time, which floods every window with
/// duplicates of the newest handful of stories.
const LOBSTERS_NEWEST: &str = "https://lobste.rs/newest";
/// ~25 stories per page; 10 pages safely spans a week.
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

/// The three tabs as DISTINCT top lists: each is the top-by-upvotes of its
/// trailing window (24h / 48h / week), minus any story already shown in a
/// shorter window, so nothing repeats across tabs. Computed shortest-first;
/// `limit` caps each window after de-overlapping. Same-id duplicates within a
/// window (e.g. a story fetched on two pages) collapse to the highest-scored
/// copy.
fn distinct_windows(
    items: &[FeedItem],
    now: DateTime<Utc>,
    limit: u32,
) -> (Vec<FeedItem>, Vec<FeedItem>, Vec<FeedItem>) {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut bucket = |hours: i64| -> Vec<FeedItem> {
        let cutoff = now - chrono::Duration::hours(hours);
        let mut hits: Vec<FeedItem> = items
            .iter()
            .filter(|i| i.published.is_some_and(|p| p >= cutoff))
            .filter(|i| i.id.as_ref().is_none_or(|id| !seen.contains(id)))
            .cloned()
            .collect();
        sort_by_score(&mut hits);
        let mut here: std::collections::HashSet<String> = std::collections::HashSet::new();
        hits.retain(|i| i.id.as_ref().is_none_or(|id| here.insert(id.clone())));
        hits.truncate(limit as usize);
        for id in hits.iter().filter_map(|i| i.id.clone()) {
            seen.insert(id);
        }
        hits
    };
    let w24 = bucket(24);
    let w48 = bucket(48);
    let wweek = bucket(WEEK_HOURS);
    (w24, w48, wweek)
}

pub async fn hacker_news(
    http: &reqwest::Client,
    limit: u32,
    now: DateTime<Utc>,
) -> Result<WidgetData, String> {
    // Every notable story (>= HN_MIN_POINTS) in the past week, then split into
    // the three distinct tabs by time + upvotes.
    let cutoff = (now - chrono::Duration::hours(WEEK_HOURS)).timestamp();
    let url = format!(
        "{ALGOLIA_SEARCH}?tags=story&numericFilters=created_at_i>{cutoff},points>={HN_MIN_POINTS}&hitsPerPage=1000"
    );
    let resp: AlgoliaResponse = get_json(http, &url)
        .await
        .map_err(|e| format!("hn algolia: {e}"))?;
    let items: Vec<FeedItem> = resp.hits.into_iter().map(algolia_item).collect();
    let (last_24h, last_48h, last_week) = distinct_windows(&items, now, limit);
    if last_24h.is_empty() && last_48h.is_empty() && last_week.is_empty() {
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
        let stories: Vec<LobstersStory> =
            get_json(http, &format!("{LOBSTERS_NEWEST}/page/{page}.json"))
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
    let (last_24h, last_48h, last_week) = distinct_windows(&items, now, limit);
    Ok(WidgetData::Posts(PostsData {
        last_24h,
        last_48h,
        last_week,
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
    fn distinct_windows_are_disjoint_top_lists() {
        let now = Utc::now();
        let s = |hours: i64, score: u64, id: &str| FeedItem {
            title: id.into(),
            published: Some(now - chrono::Duration::hours(hours)),
            score: Some(score),
            id: Some(id.into()),
            ..blank_item()
        };
        let items = vec![
            s(1, 500, "a"),        // in the last 24h
            s(2, 100, "b"),        // in the last 24h
            s(30, 400, "c"),       // 24-48h old
            s(24 * 5, 300, "d"),   // within the week
            s(24 * 9, 999, "old"), // older than a week — excluded everywhere
        ];
        let ids = |w: Vec<FeedItem>| -> Vec<String> { w.into_iter().map(|i| i.title).collect() };
        let (w24, w48, wk) = distinct_windows(&items, now, 10);
        assert_eq!(ids(w24), ["a", "b"]); // top of last 24h by score
        assert_eq!(ids(w48), ["c"]); // top of last 48h, minus the 24h items
        assert_eq!(ids(wk), ["d"]); // top of the week, minus the shorter tabs
    }

    #[test]
    fn distinct_windows_dedup_and_truncate() {
        let now = Utc::now();
        let s = |score: u64, id: &str| FeedItem {
            title: id.into(),
            published: Some(now - chrono::Duration::hours(1)),
            score: Some(score),
            id: Some(id.into()),
            ..blank_item()
        };
        // `abc` arrives twice (two pages); limit of 2 caps the window.
        let items = vec![s(10, "abc"), s(42, "abc"), s(30, "xyz"), s(20, "def")];
        let (w24, _, _) = distinct_windows(&items, now, 2);
        let titles: Vec<String> = w24.into_iter().map(|i| i.title).collect();
        // One row per id (highest-scored `abc` kept), truncated to 2 by score.
        assert_eq!(titles, ["abc", "xyz"]);
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
