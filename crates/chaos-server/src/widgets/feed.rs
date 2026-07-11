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
    let body = crate::http_util::get_body_capped(http, url.as_str(), MAX_BODY_BYTES).await?;
    let feed = feed_rs::parser::parse(body.as_slice()).map_err(|e| e.to_string())?;
    Ok(map_entries(feed))
}

/// Entry → FeedItem with the documented fallbacks: untitled entries get a
/// placeholder title, unparseable/missing links drop to None, and undated
/// entries fall back to their `updated` stamp.
fn map_entries(feed: feed_rs::model::Feed) -> Vec<FeedItem> {
    let source = feed.title.map(|t| t.content);
    feed.entries
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
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rss_entries_fall_back_on_missing_title_and_link() {
        let rss = r#"<?xml version="1.0"?>
<rss version="2.0"><channel><title>Example Blog</title>
<item><title>First</title><link>https://example.com/a</link>
<pubDate>Wed, 01 Jul 2026 10:00:00 GMT</pubDate></item>
<item><description>no title, no link</description></item>
</channel></rss>"#;
        let feed = feed_rs::parser::parse(rss.as_bytes()).expect("parse rss");
        let items = map_entries(feed);

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "First");
        assert_eq!(
            items[0].url.as_ref().map(|u| u.as_str()),
            Some("https://example.com/a")
        );
        assert_eq!(items[0].source.as_deref(), Some("Example Blog"));
        assert!(items[0].published.is_some());

        assert_eq!(items[1].title, "(untitled)");
        assert!(items[1].url.is_none());
        assert!(items[1].published.is_none());
    }

    #[test]
    fn atom_entries_fall_back_to_updated_when_unpublished() {
        let atom = r#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom"><title>Atom Feed</title>
<entry><title>Only updated</title><updated>2026-07-01T10:00:00Z</updated></entry>
</feed>"#;
        let feed = feed_rs::parser::parse(atom.as_bytes()).expect("parse atom");
        let items = map_entries(feed);

        assert_eq!(items.len(), 1);
        assert!(
            items[0].published.is_some(),
            "published falls back to updated"
        );
    }
}
