//! Direct link-aggregator access for offline use: when the chaos server is
//! unreachable the dashboard fetches HN itself via the Algolia archive API
//! (it sends CORS `*`, so plain reqwest works in browsers).
//! Produces [`PostsData`]: the top-by-upvotes links of the trailing
//! 24 h, 48 h and week.
//! The server has its own copy of this mapping in widgets/posts.rs — kept
//! separate because the server must not depend on this crate.

use chaos_domain::{Comment, FeedItem, PostThread, PostsData, strip_to_text};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use url::Url;

use crate::http::http_get_json as get_json;
use crate::http::http_get_text as get_text;

const HN_ITEM: &str = "https://news.ycombinator.com/item?id=";
const HN_ITEM_API: &str = "https://hn.algolia.com/api/v1/items/";
const ALGOLIA_SEARCH: &str = "https://hn.algolia.com/api/v1/search";
/// "Notable story" upvote floor, replacing the sparse `front_page` tag so the
/// middle tab fills and the week's biggest stories are included. Mirrors the
/// server's `HN_MIN_POINTS`.
const HN_MIN_POINTS: u32 = 50;
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

/// The top stories of a trailing window: everything published within the last
/// `hours`, by upvotes, deduped by id (highest-scored copy kept), capped at
/// `limit`. Windows are CUMULATIVE — the 48h tab is the top of the whole last
/// 48h (today's big stories included), not a 24-48h slice. Mirrors the server.
fn windowed(items: &[FeedItem], now: DateTime<Utc>, hours: i64, limit: u32) -> Vec<FeedItem> {
    let cutoff = now - chrono::Duration::hours(hours);
    let mut hits: Vec<FeedItem> = items
        .iter()
        .filter(|i| i.published.is_some_and(|p| p >= cutoff))
        .cloned()
        .collect();
    sort_by_score(&mut hits);
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    hits.retain(|i| i.id.as_ref().is_none_or(|id| seen.insert(id.clone())));
    hits.truncate(limit as usize);
    hits
}

/// The three cumulative tabs (24h / 48h / week) from one item pool.
fn windows(
    items: &[FeedItem],
    now: DateTime<Utc>,
    limit: u32,
) -> (Vec<FeedItem>, Vec<FeedItem>, Vec<FeedItem>) {
    (
        windowed(items, now, 24, limit),
        windowed(items, now, 48, limit),
        windowed(items, now, WEEK_HOURS, limit),
    )
}

/// HN window-tops via the Algolia archive API (CORS `*`, works in browsers
/// directly). One weekly `tags=story` / `points>=HN_MIN_POINTS` query,
/// split into the three cumulative tabs. Mirrors the server.
pub async fn hacker_news(
    http: &reqwest::Client,
    limit: u32,
    now: DateTime<Utc>,
) -> Result<PostsData, String> {
    let cutoff = (now - chrono::Duration::hours(WEEK_HOURS)).timestamp();
    let url = format!(
        "{ALGOLIA_SEARCH}?tags=story&numericFilters=created_at_i>{cutoff},points>={HN_MIN_POINTS}&hitsPerPage=1000"
    );
    let resp: AlgoliaResponse = get_json(http, &url)
        .await
        .map_err(|e| format!("hn algolia: {e}"))?;
    let items: Vec<FeedItem> = resp.hits.into_iter().map(algolia_item).collect();
    let (last_24h, last_48h, last_week) = windows(&items, now, limit);
    if last_24h.is_empty() && last_48h.is_empty() && last_week.is_empty() {
        return Err("no stories returned".into());
    }
    Ok(PostsData {
        last_24h,
        last_48h,
        last_week,
    })
}

/// Bucket a gathered item list into the three windows.
pub fn posts_from_items(items: &[FeedItem], limit: u32, now: DateTime<Utc>) -> PostsData {
    let (last_24h, last_48h, last_week) = windows(items, now, limit);
    PostsData {
        last_24h,
        last_48h,
        last_week,
    }
}

// ---- Direct comment-thread fetch (offline fallback, plain text) ----
//
// Mirrors the server's widgets/threads.rs tree-building, but every comment
// body / self-text is flattened to PLAIN TEXT via `strip_to_text` instead of
// `ammonia`-sanitized HTML — the offline path must never carry HTML into the
// webview. Kept in lock-step with the server mapping (project policy keeps the
// client/server parsers mirrored).

#[derive(Deserialize)]
struct HnThreadItem {
    id: i64,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    points: Option<u64>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    created_at_i: Option<i64>,
    #[serde(default)]
    children: Vec<HnThreadItem>,
}

fn hn_ts(secs: Option<i64>) -> Option<DateTime<Utc>> {
    secs.and_then(|t| DateTime::from_timestamp(t, 0))
}

fn hn_thread_comment(item: HnThreadItem) -> Comment {
    Comment {
        author: item.author,
        html: strip_to_text(item.text.as_deref().unwrap_or_default()),
        published: hn_ts(item.created_at_i),
        children: item.children.into_iter().map(hn_thread_comment).collect(),
    }
}

/// Parse Algolia's `/items/{id}` (nested tree) into a [`PostThread`] with
/// plain-text bodies. Mirror of the server's `map_hn_item`.
pub fn parse_hn_thread(json: &str) -> Result<PostThread, String> {
    let root: HnThreadItem = serde_json::from_str(json).map_err(|e| format!("hn item: {e}"))?;
    let id = root.id.to_string();
    let comments_url = format!("{HN_ITEM}{id}").parse().ok();
    let published = hn_ts(root.created_at_i);
    let body = root
        .text
        .as_deref()
        .filter(|t| !t.is_empty())
        .map(strip_to_text);
    let tree: Vec<Comment> = root.children.into_iter().map(hn_thread_comment).collect();
    Ok(PostThread {
        id,
        title: root.title.unwrap_or_else(|| "(untitled)".into()),
        url: root
            .url
            .filter(|u| !u.is_empty())
            .and_then(|u| Url::parse(&u).ok()),
        source: Some("Hacker News".into()),
        published,
        score: root.points,
        comments: None,
        comments_url,
        body,
        tree,
    })
}

/// Direct HN thread fetch via Algolia's item API (CORS `*`, works in browsers
/// directly). Mirrors `hacker_news`'s direct-access shape.
pub async fn hacker_news_thread(http: &reqwest::Client, id: &str) -> Result<PostThread, String> {
    let body = get_text(http, &format!("{HN_ITEM_API}{id}")).await?;
    parse_hn_thread(&body)
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
    fn windows_are_cumulative_top_lists() {
        let now = Utc::now();
        let s = |hours: i64, score: u64, id: &str| FeedItem {
            title: id.into(),
            published: Some(now - chrono::Duration::hours(hours)),
            score: Some(score),
            id: Some(id.into()),
            ..blank_item()
        };
        let items = vec![
            s(1, 500, "today"),      // last 24h
            s(2, 100, "today-low"),  // last 24h
            s(30, 900, "yesterday"), // 24-48h old, the biggest story
            s(24 * 5, 300, "older"), // within the week
            s(24 * 9, 999, "old"),   // older than a week — excluded everywhere
        ];
        let ids = |w: Vec<FeedItem>| -> Vec<String> { w.into_iter().map(|i| i.title).collect() };
        let (w24, w48, wk) = windows(&items, now, 10);
        // 24h: only last-day stories, by score.
        assert_eq!(ids(w24), ["today", "today-low"]);
        // 48h is CUMULATIVE: yesterday's bigger story tops it, but today's
        // stories are still there (mixed in by score), not dropped.
        assert_eq!(ids(w48), ["yesterday", "today", "today-low"]);
        // Week: everything in the last 7 days, by score.
        assert_eq!(ids(wk), ["yesterday", "today", "older", "today-low"]);
    }

    #[test]
    fn windowed_dedups_and_truncates() {
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
        let titles: Vec<String> = windowed(&items, now, 24, 2)
            .into_iter()
            .map(|i| i.title)
            .collect();
        // One row per id (highest-scored `abc` kept), truncated to 2 by score.
        assert_eq!(titles, ["abc", "xyz"]);
    }

    #[test]
    fn parsed_items_carry_a_non_empty_id() {
        let raw_hn = r#"{"title":"t","url":null,"points":1,"num_comments":0,"created_at_i":1783300000,"objectID":"9876"}"#;
        let hn = algolia_item(serde_json::from_str(raw_hn).expect("parse"));
        assert_eq!(hn.id.as_deref(), Some("9876"));
    }

    #[test]
    fn direct_hn_thread_is_plain_text() {
        let json = r#"{"id":1,"title":"Story","points":10,"url":"https://x.io",
          "author":"u","created_at_i":1700000000,
          "children":[{"id":2,"author":"a","text":"<p>top</p>","created_at_i":1700000100,
            "children":[{"id":3,"author":"b","text":"reply","created_at_i":1700000200,"children":[]}]}]}"#;
        let t = parse_hn_thread(json).unwrap();
        assert_eq!(t.title, "Story");
        assert_eq!(t.tree.len(), 1);
        assert_eq!(t.tree[0].children.len(), 1);
        // <p> stripped to plain text, no HTML carried into the offline path.
        assert_eq!(t.tree[0].html, "top");
        assert!(!t.tree[0].html.contains('<'));
        assert_eq!(t.tree[0].children[0].author.as_deref(), Some("b"));
        assert_eq!(
            t.comments_url.as_ref().map(Url::as_str),
            Some("https://news.ycombinator.com/item?id=1")
        );
    }

    #[test]
    fn posts_from_items_buckets_into_the_three_windows() {
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
        ];
        let posts = posts_from_items(&items, 10, now);
        let titles = |window: &[FeedItem]| -> Vec<String> {
            window.iter().map(|i| i.title.clone()).collect()
        };
        assert_eq!(titles(&posts.last_24h), ["fresh"]);
        assert_eq!(titles(&posts.last_48h), ["yesterday", "fresh"]);
        assert_eq!(
            titles(&posts.last_week),
            ["yesterday", "last week", "fresh"]
        );
        // The limit truncates each window independently.
        let capped = posts_from_items(&items, 1, now);
        assert_eq!(titles(&capped.last_week), ["yesterday"]);
    }
}
