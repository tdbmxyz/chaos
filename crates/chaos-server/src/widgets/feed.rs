//! RSS/Atom feed provider: fetches every configured feed concurrently,
//! merges the entries and keeps the newest ones. Hacker News and lobsters
//! are plain feeds too (hnrss.org, lobste.rs/rss), so one widget kind
//! covers all three roadmap items.

use chaos_domain::{FeedItem, WidgetData};
use url::Url;

/// Cap on a fetched feed body; anything larger is a misconfigured URL.
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

pub async fn fetch(http: &reqwest::Client, urls: &[Url], limit: u32) -> Result<WidgetData, String> {
    let results =
        futures::future::join_all(urls.iter().map(|url| fetch_one(http, url.clone()))).await;

    let mut items = Vec::new();
    let mut errors = Vec::new();
    for (url, result) in urls.iter().zip(results) {
        match result {
            Ok(mut feed_items) => items.append(&mut feed_items),
            Err(reason) => {
                tracing::warn!(%url, reason, "feed fetch failed");
                errors.push(format!("{url}: {reason}"));
            }
        }
    }
    if items.is_empty() && !errors.is_empty() {
        return Err(errors.join("; "));
    }

    // Newest first; undated entries sink to the bottom in feed order.
    items.sort_by_key(|item| std::cmp::Reverse(item.published));
    items.truncate(limit as usize);
    Ok(WidgetData::Feed { items })
}

async fn fetch_one(http: &reqwest::Client, url: Url) -> Result<Vec<FeedItem>, String> {
    let resp = http.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("status {}", resp.status()));
    }
    let body = resp.bytes().await.map_err(|e| e.to_string())?;
    if body.len() > MAX_BODY_BYTES {
        return Err(format!("feed body too large ({} bytes)", body.len()));
    }

    let feed = feed_rs::parser::parse(body.as_ref()).map_err(|e| e.to_string())?;
    let source = feed.title.map(|t| t.content);

    Ok(feed
        .entries
        .into_iter()
        .map(|entry| FeedItem {
            title: entry
                .title
                .map(|t| t.content)
                .unwrap_or_else(|| "(untitled)".into()),
            url: entry.links.first().and_then(|l| l.href.parse().ok()),
            source: source.clone(),
            published: entry.published.or(entry.updated),
            score: None,
            comments: None,
            comments_url: None,
        })
        .collect())
}
