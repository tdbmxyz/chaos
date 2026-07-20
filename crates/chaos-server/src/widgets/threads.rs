//! Comment-thread fetch + sanitize for the reader endpoint.
//!
//! Hacker News comes from Algolia's item API (`/api/v1/items/{id}`), whose
//! response is already a nested tree. Lobsters' `/s/{id}.json` returns a flat,
//! pre-ordered comment list with 1-based `depth`, rebuilt here into a tree with
//! a depth stack. Every body is run through [`sanitize_html`] (an `ammonia`
//! allowlist) so the client can render it as `inner_html` online.

use std::collections::HashSet;

use chaos_domain::{Comment, PostThread};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use url::Url;

const HN_ITEM_API: &str = "https://hn.algolia.com/api/v1/items/";
const HN_ITEM: &str = "https://news.ycombinator.com/item?id=";
const LOBSTERS_STORY: &str = "https://lobste.rs/s/";

/// Allowlist-sanitize provider comment HTML down to safe inline/block markup.
/// Only `a[href]` with `http`/`https` survives; anchors get
/// `rel="noreferrer noopener"`. Scripts, event handlers and everything else
/// are dropped.
pub(crate) fn sanitize_html(dirty: &str) -> String {
    ammonia::Builder::new()
        .tags(HashSet::from([
            "a",
            "p",
            "i",
            "em",
            "b",
            "strong",
            "code",
            "pre",
            "blockquote",
            "br",
        ]))
        .link_rel(Some("noreferrer noopener"))
        .add_tag_attributes("a", &["href"])
        .url_schemes(HashSet::from(["http", "https"]))
        .clean(dirty)
        .to_string()
}

// ---- Hacker News (Algolia item API) ----

#[derive(Deserialize)]
struct HnItem {
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
    children: Vec<HnItem>,
}

fn hn_ts(secs: Option<i64>) -> Option<DateTime<Utc>> {
    secs.and_then(|t| DateTime::from_timestamp(t, 0))
}

fn hn_comment(item: HnItem) -> Comment {
    Comment {
        author: item.author,
        html: sanitize_html(item.text.as_deref().unwrap_or_default()),
        published: hn_ts(item.created_at_i),
        children: item.children.into_iter().map(hn_comment).collect(),
    }
}

pub(crate) fn map_hn_item(json: &str) -> Result<PostThread, String> {
    let root: HnItem = serde_json::from_str(json).map_err(|e| format!("hn item: {e}"))?;
    let id = root.id.to_string();
    let comments_url = format!("{HN_ITEM}{id}").parse().ok();
    let published = hn_ts(root.created_at_i);
    let body = root
        .text
        .as_deref()
        .filter(|t| !t.is_empty())
        .map(sanitize_html);
    let tree: Vec<Comment> = root.children.into_iter().map(hn_comment).collect();
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

pub(crate) async fn fetch_hn(http: &reqwest::Client, id: &str) -> Result<PostThread, String> {
    let body = crate::http_util::get_text(http, &format!("{HN_ITEM_API}{id}")).await?;
    map_hn_item(&body)
}

// ---- Lobsters (/s/{id}.json) ----

#[derive(Deserialize)]
struct LobstersStoryFull {
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
    comments: Vec<LobstersComment>,
}

#[derive(Deserialize)]
struct LobstersComment {
    #[serde(default)]
    comment: String,
    depth: usize,
    #[serde(default)]
    commenting_user: Option<String>,
    #[serde(default)]
    created_at: Option<DateTime<Utc>>,
}

/// Rebuild a tree from the flat, pre-ordered depth list. `depth` is 1-based;
/// each item attaches under the last item seen at `depth - 1`.
fn lobsters_tree(comments: Vec<LobstersComment>) -> Vec<Comment> {
    let mut arena: Vec<Option<Comment>> = Vec::with_capacity(comments.len());
    let mut children: Vec<Vec<usize>> = Vec::with_capacity(comments.len());
    let mut roots: Vec<usize> = Vec::new();
    // `stack[i]` is the ancestor at depth `i + 1`.
    let mut stack: Vec<usize> = Vec::new();
    for c in comments {
        let idx = arena.len();
        arena.push(Some(Comment {
            author: c.commenting_user,
            html: sanitize_html(&c.comment),
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

pub(crate) fn map_lobsters_story(json: &str) -> Result<PostThread, String> {
    let story: LobstersStoryFull =
        serde_json::from_str(json).map_err(|e| format!("lobsters story: {e}"))?;
    let body = story
        .description
        .as_deref()
        .filter(|t| !t.is_empty())
        .map(sanitize_html);
    let comments_url = story
        .comments_url
        .clone()
        .or_else(|| format!("{LOBSTERS_STORY}{}", story.short_id).parse().ok());
    let tree = lobsters_tree(story.comments);
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

pub(crate) async fn fetch_lobsters(http: &reqwest::Client, id: &str) -> Result<PostThread, String> {
    let body = crate::http_util::get_text(http, &format!("{LOBSTERS_STORY}{id}.json")).await?;
    map_lobsters_story(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hn_item_maps_nested_tree() {
        let json = r#"{"id":1,"title":"Story","points":10,"url":"https://x.io",
          "author":"u","created_at_i":1700000000,
          "children":[{"id":2,"author":"a","text":"<p>top</p>","created_at_i":1700000100,
            "children":[{"id":3,"author":"b","text":"reply","created_at_i":1700000200,"children":[]}]}]}"#;
        let t = map_hn_item(json).unwrap();
        assert_eq!(t.title, "Story");
        assert_eq!(t.tree.len(), 1);
        assert_eq!(t.tree[0].children.len(), 1);
        assert_eq!(t.tree[0].children[0].author.as_deref(), Some("b"));
        assert_eq!(t.tree[0].author.as_deref(), Some("a"));
        assert_eq!(
            t.comments_url.as_ref().map(Url::as_str),
            Some("https://news.ycombinator.com/item?id=1")
        );
    }

    #[test]
    fn lobsters_depth_list_rebuilds_tree() {
        // lobste.rs /s/{id}.json: flat comments with `depth` (1-based) in pre-order.
        let json = r#"{"short_id":"abc","title":"S","score":4,"url":"https://x.io",
          "comments":[
            {"short_id":"c1","comment":"a","depth":1,"commenting_user":"u1","created_at":"2024-01-01T00:00:00Z"},
            {"short_id":"c2","comment":"b","depth":2,"commenting_user":"u2","created_at":"2024-01-01T00:01:00Z"},
            {"short_id":"c3","comment":"c","depth":1,"commenting_user":"u3","created_at":"2024-01-01T00:02:00Z"}]}"#;
        let t = map_lobsters_story(json).unwrap();
        assert_eq!(t.tree.len(), 2); // c1, c3 at top level
        assert_eq!(t.tree[0].children.len(), 1); // c2 under c1
        assert_eq!(t.tree[0].author.as_deref(), Some("u1"));
        assert_eq!(t.tree[1].author.as_deref(), Some("u3"));
    }

    #[test]
    fn sanitize_strips_script_keeps_links() {
        let dirty = r#"<p onclick="x">hi</p><script>evil()</script><a href="https://x.io">l</a>"#;
        let clean = sanitize_html(dirty);
        assert!(!clean.contains("script"));
        assert!(!clean.contains("onclick"));
        assert!(clean.contains("href=\"https://x.io\""));
        assert!(clean.contains("rel=") && clean.contains("noreferrer"));
    }
}
