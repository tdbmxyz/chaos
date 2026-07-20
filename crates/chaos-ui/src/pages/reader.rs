//! The in-app comment reader (`/news/:source/:id`): a story header plus a
//! collapsible comment tree. Online it renders the server-sanitized HTML
//! thread (`client.post_thread`); offline it fetches the provider directly
//! (HN's Algolia item API in browser/shell alike, lobste.rs through the shell
//! HTTP plugin) and renders PLAIN TEXT.
//!
//! SECURITY: `inner_html` renders a body ONLY when it is server-sanitized. That
//! provenance is decided at fetch time and travels WITH the loaded thread (the
//! `sanitized` bool from [`load_thread`]) — it is never re-derived from the live
//! connectivity signal, which would desync from a stale-while-revalidate value.
//! The offline direct path carries plain text (whose entities `strip_to_text`
//! decodes back into live markup), so it is rendered as a Leptos text node
//! (escaped `textContent`) and NEVER as `inner_html`. The server and direct
//! paths use DISTINCT cache keys so a stale server-cache read can never surface
//! a plain-text body marked sanitized. Titles and authors are always text nodes.

use chaos_client::ChaosClient;
use chaos_domain::{Comment, PostThread, Source};
use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;

use super::dashboard::{rel_time, score_color};
use crate::offline::Connectivity;
use crate::use_client;

/// Cache key for the SERVER path (sanitized HTML); only ever written by
/// `client.post_thread`, so a stale read from it is always sanitized.
fn server_thread_key(source: Source, id: &str) -> String {
    format!("thread:{}:{id}", source.as_str())
}

/// Cache key for the DIRECT path (plain text); kept distinct from the server
/// key so the two provenances can never contaminate each other.
fn direct_thread_key(source: Source, id: &str) -> String {
    format!("thread-direct:{}:{id}", source.as_str())
}

/// Total number of descendants under `node` (for the `[+N]` collapse badge).
fn count_descendants(node: &Comment) -> usize {
    node.children.len() + node.children.iter().map(count_descendants).sum::<usize>()
}

/// Load the thread: online through the chaos server (sanitized HTML, cached
/// under `thread:{source}:{id}`); offline direct from the provider, falling
/// back to the cached copy. Mirrors `dashboard::load_posts`.
async fn load_thread(
    source: Source,
    id: &str,
    conn: RwSignal<Connectivity>,
    client: &ChaosClient,
) -> Result<(PostThread, bool), String> {
    // `bool` = SANITIZED: true only for the server path, so the caller's
    // `inner_html` decision travels with the data instead of being re-read from
    // the live `conn` signal (which can desync from a stale resource value).
    if conn.get_untracked() != Connectivity::Online {
        return thread_direct(source, id)
            .await
            .map(|thread| (thread, false));
    }
    let key = server_thread_key(source, id);
    match crate::offline::cached(conn, &key, async { client.post_thread(source, id).await }).await {
        Ok((thread, _stale)) => Ok((thread, true)),
        Err(err) => Err(err.to_string()),
    }
}

/// The offline direct path: fetch from the provider, overwrite the (direct)
/// cache on success, fall back to the cached copy on failure. Bodies are plain
/// text, cached under a key distinct from the server path's.
async fn thread_direct(source: Source, id: &str) -> Result<PostThread, String> {
    let key = direct_thread_key(source, id);
    let fetched = match source {
        Source::HackerNews => {
            chaos_client::posts::hacker_news_thread(&crate::weather_fetch::http(), id).await
        }
        Source::Lobsters => lobsters_thread_direct(id).await,
    };
    match fetched {
        Ok(thread) => {
            crate::offline::cache_put(&key, &thread);
            Ok(thread)
        }
        Err(err) => crate::offline::cache_get::<PostThread>(&key).ok_or(err),
    }
}

/// lobste.rs sends no CORS headers, so the story JSON must come through the
/// shell's HTTP plugin (unavailable in a plain browser).
async fn lobsters_thread_direct(id: &str) -> Result<PostThread, String> {
    use chaos_client::posts;
    match crate::tauri_http::fetch_text(&posts::lobsters_thread_url(id)).await {
        Some(Ok(json)) => posts::parse_lobsters_thread(&json),
        Some(Err(err)) => Err(err),
        None => Err("lobsters needs the app shell offline".into()),
    }
}

/// The `fav:` icon-proxy spec for a thread: the article host when there is a
/// link, else the provider's own host.
fn thread_favicon_spec(thread: &PostThread, source: Source) -> String {
    let host = thread
        .url
        .as_ref()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| {
            match source {
                Source::HackerNews => "news.ycombinator.com",
                Source::Lobsters => "lobste.rs",
            }
            .to_owned()
        });
    format!("fav:{host}")
}

#[component]
pub fn PostReader() -> impl IntoView {
    let client = use_client();
    let conn = crate::offline::use_connectivity();
    let params = use_params_map();
    let source = move || {
        params
            .read()
            .get("source")
            .and_then(|s| Source::from_str(&s))
    };
    let id = move || params.read().get("id").unwrap_or_default();

    let thread = LocalResource::new({
        let client = client.clone();
        move || {
            let client = client.clone();
            let (src, id) = (source(), id());
            conn.track(); // recovery re-fetches
            async move {
                match src {
                    Some(s) => load_thread(s, &id, conn, &client).await,
                    None => Err("unknown source".to_string()),
                }
            }
        }
    });

    view! {
        <section class="reader">
            <A href="/news" attr:class="reader-back">"‹ News"</A>
            {move || match thread.get() {
                None => view! { <p class="muted">"Loading…"</p> }.into_any(),
                Some(Err(err)) => view! { <p class="error">{err}</p> }.into_any(),
                Some(Ok((t, sanitized))) => {
                    let src = source().unwrap_or(Source::HackerNews);
                    reader_body(t, sanitized, src, client.clone()).into_any()
                }
            }}
        </section>
    }
}

/// The loaded story header, optional self-text, and the comment tree.
fn reader_body(
    thread: PostThread,
    sanitized: bool,
    source: Source,
    client: ChaosClient,
) -> impl IntoView {
    let fav_url = client
        .icon_url(&thread_favicon_spec(&thread, source))
        .map(|u| u.to_string())
        .unwrap_or_default();
    // A single score has no percentile to scale against; color it against
    // itself (→ hard red), which the spec accepts for a lone score.
    let score = thread.score.map(|s| format!("▲ {s}"));
    let score_style = thread.score.map(|s| score_color(s, s));
    let comments = thread
        .comments
        .map(|n| format!("{n} comment{}", if n == 1 { "" } else { "s" }));
    let age = thread.published.map(rel_time);
    // Title is ALWAYS a text node (never inner_html).
    let title = thread.title.clone();
    let title_view = match thread.url.as_ref().map(|u| u.to_string()) {
        Some(href) => view! {
            <a class="reader-title" href=href target="_blank" rel="noreferrer">{title}</a>
        }
        .into_any(),
        None => view! { <span class="reader-title">{title}</span> }.into_any(),
    };
    // Self-text body: inner_html only for server-sanitized HTML; text otherwise.
    let body_view = thread.body.map(|body| {
        if sanitized {
            view! { <div class="reader-selftext comment-body" inner_html=body></div> }.into_any()
        } else {
            view! { <div class="reader-selftext comment-body">{body}</div> }.into_any()
        }
    });
    let tree = thread.tree;

    view! {
        <header class="reader-head">
            <div class="reader-headline">
                {title_view}
                <img class="reader-favicon" src=fav_url alt="" loading="lazy" />
            </div>
            <div class="muted feed-meta">
                <span class="feed-score" style:color=score_style>{score}</span>
                <span class="feed-comments">{comments}</span>
                <span class="feed-age">{age}</span>
            </div>
        </header>
        {body_view}
        <ul class="comment-children reader-tree">
            {tree.into_iter().map(|c| comment_view(c, sanitized)).collect_view()}
        </ul>
    }
}

/// One comment and its subtree, with per-node collapse: clicking the header
/// folds this node's own subtree (showing a `[+N]` descendant badge); clicking
/// again expands it.
fn comment_view(node: Comment, sanitized: bool) -> AnyView {
    let collapsed = RwSignal::new(false);
    let child_count = count_descendants(&node);
    // Author is ALWAYS a text node (never inner_html).
    let author = node.author.clone().unwrap_or_else(|| "[deleted]".into());
    let age = node.published.map(rel_time).unwrap_or_default();
    let meta = format!("{author} · {age}");
    let body = node.html.clone();
    let children = node.children;

    view! {
        <li class="comment">
            <div class="comment-head" on:click=move |_| collapsed.update(|c| *c = !*c)>
                <span class="comment-meta">{meta}</span>
                {move || {
                    collapsed
                        .get()
                        .then(|| view! { <span class="comment-badge">{format!("[+{child_count}]")}</span> })
                }}
            </div>
            <Show when=move || !collapsed.get()>
                {
                    let body = body.clone();
                    // inner_html ONLY for server-sanitized HTML; text otherwise.
                    if sanitized {
                        view! { <div class="comment-body" inner_html=body></div> }.into_any()
                    } else {
                        view! { <div class="comment-body">{body}</div> }.into_any()
                    }
                }
                <ul class="comment-children">
                    {children.clone().into_iter().map(|c| comment_view(c, sanitized)).collect_view()}
                </ul>
            </Show>
        </li>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(html: &str) -> Comment {
        Comment {
            author: None,
            html: html.into(),
            published: None,
            children: vec![],
        }
    }

    #[test]
    fn server_and_direct_cache_keys_are_distinct() {
        // The provenance safety rests on these never colliding: a stale read
        // from the server key must never surface a plain-text (direct) body.
        let s = server_thread_key(Source::HackerNews, "42");
        let d = direct_thread_key(Source::HackerNews, "42");
        assert_ne!(s, d);
        assert!(
            !d.starts_with(&s),
            "direct key must not be a prefix-collision of the server key"
        );
    }

    #[test]
    fn count_descendants_sums_the_whole_subtree() {
        // root
        //  ├ a
        //  │  └ a1
        //  └ b
        // 3 descendants under root, 1 under `a`, 0 under leaves.
        let a = Comment {
            children: vec![leaf("a1")],
            ..leaf("a")
        };
        let root = Comment {
            children: vec![a, leaf("b")],
            ..leaf("root")
        };
        assert_eq!(count_descendants(&root), 3);
        assert_eq!(count_descendants(&root.children[0]), 1);
        assert_eq!(count_descendants(&root.children[1]), 0);
    }
}
