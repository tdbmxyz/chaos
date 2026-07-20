//! Direct link-aggregator access for offline use: when the chaos server is
//! unreachable the dashboard fetches HN itself via the Algolia archive API
//! (it sends CORS `*`, so plain reqwest works in browsers and shells alike)
//! and parses lobsters `newest.json` pages fetched through the Tauri HTTP
//! plugin (lobste.rs sends no CORS headers, so browsers can't fetch it —
//! the UI passes the raw page bodies in from `window.__TAURI__.http.fetch`).
//! Both produce [`PostsData`]: the top-by-upvotes links of the trailing
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
const LOBSTERS_STORY: &str = "https://lobste.rs/s/";
const ALGOLIA_SEARCH: &str = "https://hn.algolia.com/api/v1/search";
/// Base for the paginated newest feed. The page number goes in the PATH
/// (`/newest/page/{n}.json`) — the `?page=` query form is silently ignored by
/// lobste.rs and returns page 1 every time.
const LOBSTERS_NEWEST: &str = "https://lobste.rs/newest";
/// ~25 stories per page; 10 pages safely spans a week.
pub const LOBSTERS_PAGE_CAP: u32 = 10;
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
/// Drop later duplicates that share an `id`, keeping the first (after sorting,
/// the highest-scored copy). Items without an id are always kept. Defends
/// against the same story arriving on more than one fetched page.
fn dedup_by_id(items: &mut Vec<FeedItem>) {
    let mut seen = std::collections::HashSet::new();
    items.retain(|i| match &i.id {
        Some(id) => seen.insert(id.clone()),
        None => true,
    });
}

fn windowed(items: &[FeedItem], now: DateTime<Utc>, hours: i64, limit: u32) -> Vec<FeedItem> {
    let cutoff = now - chrono::Duration::hours(hours);
    let mut hits: Vec<FeedItem> = items
        .iter()
        .filter(|i| i.published.is_some_and(|p| p >= cutoff))
        .cloned()
        .collect();
    sort_by_score(&mut hits);
    dedup_by_id(&mut hits);
    hits.truncate(limit as usize);
    hits
}

/// HN window-tops via the Algolia archive API (CORS `*`, works in browsers
/// and shells alike). Same three-query shape as the server; sequential
/// awaits, which is fine single-threaded on wasm.
pub async fn hacker_news(
    http: &reqwest::Client,
    limit: u32,
    now: DateTime<Utc>,
) -> Result<PostsData, String> {
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
    let last_24h = window(24).await?;
    let last_48h = window(48).await?;
    let last_week = window(WEEK_HOURS).await?;
    if last_week.is_empty() {
        return Err("no stories returned".into());
    }
    Ok(PostsData {
        last_24h,
        last_48h,
        last_week,
    })
}

/// One page of `newest.json` (fetched by the caller — browsers need the
/// Tauri HTTP plugin for lobste.rs) parsed to items, page order kept.
pub fn parse_lobsters_page(json: &str) -> Result<Vec<FeedItem>, String> {
    let stories: Vec<LobstersStory> =
        serde_json::from_str(json).map_err(|e| format!("lobsters: {e}"))?;
    Ok(stories.into_iter().map(lobsters_item).collect())
}

/// Where page `n` of the lobsters sweep lives.
pub fn lobsters_page_url(page: u32) -> String {
    format!("{LOBSTERS_NEWEST}/page/{page}.json")
}

/// Sweep stop test: newest-first pages — stop once the oldest (last) story
/// on the page predates now-7d (or has no date), since later pages are all
/// older.
pub fn lobsters_page_done(items: &[FeedItem], now: DateTime<Utc>) -> bool {
    let cutoff = now - chrono::Duration::hours(WEEK_HOURS);
    items
        .last()
        .and_then(|i| i.published)
        .is_none_or(|t| t < cutoff)
}

/// Bucket a gathered item list into the three windows.
pub fn posts_from_items(items: &[FeedItem], limit: u32, now: DateTime<Utc>) -> PostsData {
    PostsData {
        last_24h: windowed(items, now, 24, limit),
        last_48h: windowed(items, now, 48, limit),
        last_week: windowed(items, now, WEEK_HOURS, limit),
    }
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

#[derive(Deserialize)]
struct LobstersThreadStory {
    short_id: String,
    title: String,
    #[serde(default)]
    score: i64,
    #[serde(default)]
    url: String,
    #[serde(default)]
    comments_url: Option<Url>,
    #[serde(default)]
    created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    comment_count: Option<u64>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    comments: Vec<LobstersThreadComment>,
}

#[derive(Deserialize)]
struct LobstersThreadComment {
    #[serde(default)]
    comment: String,
    depth: usize,
    #[serde(default)]
    commenting_user: Option<String>,
    #[serde(default)]
    created_at: Option<DateTime<Utc>>,
}

/// Rebuild a tree from the flat, pre-ordered depth list. `depth` is 1-based;
/// each item attaches under the last item seen at `depth - 1`. Mirror of the
/// server's `lobsters_tree`, plain-text bodies.
fn lobsters_thread_tree(comments: Vec<LobstersThreadComment>) -> Vec<Comment> {
    let mut arena: Vec<Option<Comment>> = Vec::with_capacity(comments.len());
    let mut children: Vec<Vec<usize>> = Vec::with_capacity(comments.len());
    let mut roots: Vec<usize> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for c in comments {
        let idx = arena.len();
        arena.push(Some(Comment {
            author: c.commenting_user,
            html: strip_to_text(&c.comment),
            published: c.created_at,
            children: Vec::new(),
        }));
        children.push(Vec::new());
        let depth = c.depth.max(1);
        stack.truncate(depth - 1);
        match stack.last() {
            Some(&parent) => children[parent].push(idx),
            None => roots.push(idx),
        }
        stack.push(idx);
    }
    roots
        .into_iter()
        .map(|r| assemble(r, &mut arena, &children))
        .collect()
}

fn assemble(idx: usize, arena: &mut Vec<Option<Comment>>, children: &[Vec<usize>]) -> Comment {
    let mut node = arena[idx].take().expect("each node assembled once");
    node.children = children[idx]
        .iter()
        .map(|&c| assemble(c, arena, children))
        .collect();
    node
}

/// Parse lobste.rs `/s/{id}.json` (flat depth list) into a [`PostThread`] with
/// plain-text bodies. Mirror of the server's `map_lobsters_story`.
pub fn parse_lobsters_thread(json: &str) -> Result<PostThread, String> {
    let story: LobstersThreadStory =
        serde_json::from_str(json).map_err(|e| format!("lobsters story: {e}"))?;
    let body = story
        .description
        .as_deref()
        .filter(|t| !t.is_empty())
        .map(strip_to_text);
    let comments_url = story
        .comments_url
        .clone()
        .or_else(|| format!("{LOBSTERS_STORY}{}", story.short_id).parse().ok());
    let tree = lobsters_thread_tree(story.comments);
    Ok(PostThread {
        id: story.short_id,
        title: story.title,
        url: Some(story.url)
            .filter(|u| !u.is_empty())
            .and_then(|u| Url::parse(&u).ok())
            .or_else(|| comments_url.clone()),
        source: Some("Lobsters".into()),
        published: story.created_at,
        score: u64::try_from(story.score).ok(),
        comments: story.comment_count,
        comments_url,
        body,
        tree,
    })
}

/// Direct HN thread fetch via Algolia's item API (CORS `*`, works in browsers
/// and shells alike). Mirrors `hacker_news`'s direct-access shape.
pub async fn hacker_news_thread(http: &reqwest::Client, id: &str) -> Result<PostThread, String> {
    let body = get_text(http, &format!("{HN_ITEM_API}{id}")).await?;
    parse_hn_thread(&body)
}

/// Where a lobsters story's JSON lives (fetched by the caller — browsers need
/// the Tauri HTTP plugin for lobste.rs). Mirrors `lobsters_page_url`.
pub fn lobsters_thread_url(id: &str) -> String {
    format!("{LOBSTERS_STORY}{id}.json")
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
    fn windowed_dedups_by_id_keeping_the_best() {
        let now = Utc::now();
        let dup = |score: u64, id: &str, title: &str| FeedItem {
            title: title.into(),
            published: Some(now - chrono::Duration::hours(1)),
            score: Some(score),
            id: Some(id.into()),
            ..blank_item()
        };
        // The same story arriving on two pages, plus a distinct one.
        let items = vec![
            dup(10, "abc", "abc low copy"),
            dup(42, "abc", "abc high copy"),
            dup(30, "xyz", "other"),
        ];
        let titles: Vec<String> = windowed(&items, now, 24, 10)
            .into_iter()
            .map(|i| i.title)
            .collect();
        // One row per id; the higher-scored copy of `abc` is the survivor.
        assert_eq!(titles, ["abc high copy", "other"]);
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
    fn lobsters_text_post_falls_back_to_discussion() {
        let raw = r#"{"short_id":"x","title":"Meta","url":"","score":5,"comment_count":2,"comments_url":"https://lobste.rs/s/x/meta","created_at":"2026-07-06T01:02:03Z"}"#;
        let item = lobsters_item(serde_json::from_str(raw).expect("parse"));
        assert_eq!(item.url.unwrap().as_str(), "https://lobste.rs/s/x/meta");
    }

    #[test]
    fn parses_a_raw_lobsters_page_array() {
        let raw = r#"[
            {"short_id":"a","title":"first","url":"https://e.org/1","score":2,"comment_count":0,"comments_url":"https://lobste.rs/s/a","created_at":"2026-07-06T01:02:03Z"},
            {"short_id":"b","title":"second","url":"https://e.org/2","score":50,"comment_count":1,"comments_url":"https://lobste.rs/s/b","created_at":"2026-07-05T01:02:03Z"}
        ]"#;
        let items = parse_lobsters_page(raw).unwrap();
        // Page order preserved (newest first); sorting happens per window later.
        let titles: Vec<_> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["first", "second"]);
        assert_eq!(items[1].score, Some(50));
    }

    #[test]
    fn parsed_items_carry_a_non_empty_id() {
        let raw_hn = r#"{"title":"t","url":null,"points":1,"num_comments":0,"created_at_i":1783300000,"objectID":"9876"}"#;
        let hn = algolia_item(serde_json::from_str(raw_hn).expect("parse"));
        assert_eq!(hn.id.as_deref(), Some("9876"));
        let page = r#"[
            {"short_id":"abc123","title":"first","url":"https://e.org/1","score":2,"comment_count":0,"comments_url":"https://lobste.rs/s/abc123","created_at":"2026-07-06T01:02:03Z"}
        ]"#;
        let items = parse_lobsters_page(page).unwrap();
        assert_eq!(items[0].id.as_deref(), Some("abc123"));
        assert!(items[0].id.as_ref().is_some_and(|id| !id.is_empty()));
    }

    #[test]
    fn rejects_a_malformed_lobsters_page() {
        assert!(parse_lobsters_page("{\"not\":\"an array\"}").is_err());
    }

    #[test]
    fn lobsters_page_urls_are_numbered() {
        assert_eq!(lobsters_page_url(1), "https://lobste.rs/newest/page/1.json");
        assert_eq!(lobsters_page_url(7), "https://lobste.rs/newest/page/7.json");
    }

    #[test]
    fn page_done_once_the_oldest_story_predates_the_week() {
        let now = Utc::now();
        let aged = |hours: i64| FeedItem {
            published: Some(now - chrono::Duration::hours(hours)),
            ..blank_item()
        };
        // Oldest (last) story still inside the week: keep paging.
        assert!(!lobsters_page_done(&[aged(1), aged(100)], now));
        // Oldest story past the cutoff: the sweep covers the week.
        assert!(lobsters_page_done(&[aged(1), aged(WEEK_HOURS + 1)], now));
        // No date on the oldest story: stop rather than page forever.
        assert!(lobsters_page_done(&[aged(1), blank_item()], now));
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
    fn direct_lobsters_thread_is_plain_text_tree() {
        let json = r#"{"short_id":"abc","title":"S","score":4,"url":"https://x.io",
          "comments":[
            {"comment":"<p>a &amp; b</p>","depth":1,"commenting_user":"u1","created_at":"2024-01-01T00:00:00Z"},
            {"comment":"b","depth":2,"commenting_user":"u2","created_at":"2024-01-01T00:01:00Z"},
            {"comment":"c","depth":1,"commenting_user":"u3","created_at":"2024-01-01T00:02:00Z"}]}"#;
        let t = parse_lobsters_thread(json).unwrap();
        assert_eq!(t.tree.len(), 2); // c1, c3 at top level
        assert_eq!(t.tree[0].children.len(), 1); // c2 under c1
        assert_eq!(t.tree[0].author.as_deref(), Some("u1"));
        // Tags stripped, entities decoded — plain text, no `<`.
        assert_eq!(t.tree[0].html, "a & b");
        assert!(!t.tree[0].html.contains('<'));
    }

    #[test]
    fn lobsters_thread_url_points_at_the_story_json() {
        assert_eq!(
            lobsters_thread_url("abc123"),
            "https://lobste.rs/s/abc123.json"
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
