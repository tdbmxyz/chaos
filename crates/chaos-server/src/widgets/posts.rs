//! Link-aggregator providers: Hacker News and Lobsters, with points and
//! comment counts (their RSS feeds carry neither, hence not `feed.rs`).
//!
//! Hacker News: the official Firebase API. `topstories.json` is the live
//! front-page ranking ("trending"), NOT newest-first; items are fetched
//! concurrently. Lobsters: one request to `hottest.json`.

use chaos_domain::{FeedItem, WidgetData};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use url::Url;

const HN_API: &str = "https://hacker-news.firebaseio.com/v0";
const HN_ITEM: &str = "https://news.ycombinator.com/item?id=";
const LOBSTERS_HOTTEST: &str = "https://lobste.rs/hottest.json";

#[derive(Deserialize)]
struct HnItem {
    id: u64,
    title: Option<String>,
    url: Option<Url>,
    score: Option<u64>,
    /// Total comment count, unlike `kids` (direct replies only).
    descendants: Option<u64>,
    time: Option<i64>,
}

#[derive(Deserialize)]
struct LobstersStory {
    title: String,
    url: String,
    score: i64,
    comment_count: u64,
    comments_url: Url,
    created_at: Option<DateTime<Utc>>,
}

pub async fn hacker_news(http: &reqwest::Client, limit: u32) -> Result<WidgetData, String> {
    let ids: Vec<u64> = get_json(http, &format!("{HN_API}/topstories.json")).await?;

    let items = futures::future::join_all(ids.iter().take(limit as usize).map(|id| {
        let url = format!("{HN_API}/item/{id}.json");
        async move { get_json::<HnItem>(http, &url).await }
    }))
    .await
    .into_iter()
    // A single dead/deleted item must not take the widget down.
    .filter_map(|result| result.ok().map(hn_item))
    .collect::<Vec<_>>();

    if items.is_empty() {
        return Err("no stories returned".into());
    }
    Ok(WidgetData::Feed { items })
}

fn hn_item(item: HnItem) -> FeedItem {
    let discussion: Option<Url> = format!("{HN_ITEM}{}", item.id).parse().ok();
    FeedItem {
        title: item.title.unwrap_or_else(|| "(untitled)".into()),
        // Ask HN / Show HN text posts have no external URL.
        url: item.url.or_else(|| discussion.clone()),
        source: Some("Hacker News".into()),
        published: item.time.and_then(|t| DateTime::from_timestamp(t, 0)),
        score: item.score,
        comments: item.descendants,
        comments_url: discussion,
    }
}

pub async fn lobsters(http: &reqwest::Client, limit: u32) -> Result<WidgetData, String> {
    let stories: Vec<LobstersStory> = get_json(http, LOBSTERS_HOTTEST).await?;
    let items = stories
        .into_iter()
        .take(limit as usize)
        .map(lobsters_item)
        .collect();
    Ok(WidgetData::Feed { items })
}

fn lobsters_item(story: LobstersStory) -> FeedItem {
    let discussion = story.comments_url;
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
    }
}

async fn get_json<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    url: &str,
) -> Result<T, String> {
    let resp = http.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("{url}: status {}", resp.status()));
    }
    resp.json().await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_a_hacker_news_item() {
        let raw = r#"{"id":1234,"title":"Rust 2.0","url":"https://blog.rust-lang.org/2","score":256,"descendants":142,"time":1783300000,"by":"pcwalton","type":"story","kids":[1,2]}"#;
        let item = hn_item(serde_json::from_str(raw).expect("parse"));
        assert_eq!(item.title, "Rust 2.0");
        assert_eq!(item.score, Some(256));
        assert_eq!(item.comments, Some(142));
        assert_eq!(
            item.comments_url.as_ref().map(Url::as_str),
            Some("https://news.ycombinator.com/item?id=1234")
        );
        assert_eq!(item.url.unwrap().as_str(), "https://blog.rust-lang.org/2");
        assert!(item.published.is_some());
    }

    #[test]
    fn hn_text_post_links_to_the_discussion() {
        let raw =
            r#"{"id":42,"title":"Ask HN: editors?","score":10,"descendants":3,"time":1783300000}"#;
        let item = hn_item(serde_json::from_str(raw).expect("parse"));
        assert_eq!(item.url, item.comments_url);
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
}
