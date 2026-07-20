use chaos_domain::Source;
use leptos::prelude::*;

use super::dashboard::{PostsTab, load_posts, post_row_view, posts_window, score_anchor};
use crate::use_client;

/// The trailing window a range index selects (0=24h, 1=48h, 2=week).
fn range_tab(idx: u8) -> PostsTab {
    match idx {
        0 => PostsTab::Day,
        1 => PostsTab::TwoDays,
        _ => PostsTab::Week,
    }
}

/// The dedicated news reader page: HN / lobste.rs sub-tabs, a 24h/48h/Week
/// range strip, and favicon rows (via `post_row_view`) whose titles open the
/// in-app reader. The selected source and range persist per device.
#[component]
pub fn NewsPage() -> impl IntoView {
    let client = use_client();
    let conn = crate::offline::use_connectivity();
    let source = RwSignal::new(crate::news_source());
    let range = RwSignal::new(crate::news_range());

    // Persist the choices as they change, so the page reopens where it left.
    Effect::new(move |_| crate::set_news_source(source.get()));
    Effect::new(move |_| crate::set_news_range(range.get()));

    let data = LocalResource::new({
        let client = client.clone();
        move || {
            let client = client.clone();
            let source = source.get();
            conn.track(); // recovery re-fetches
            async move { load_posts(source, conn, &client).await }
        }
    });

    // Top-level Memo (owner scope, not nested in a type-erased match arm) so a
    // range click actually re-renders the list. Carries the union anchor so
    // switching windows never rescales the score colors.
    let window = Memo::new(move |_| {
        data.get().and_then(|r| r.ok()).map(|(posts, _)| {
            let anchor = score_anchor(
                posts
                    .last_24h
                    .iter()
                    .chain(&posts.last_48h)
                    .chain(&posts.last_week)
                    .map(|i| i.score),
            );
            (posts_window(&posts, range_tab(range.get())), anchor)
        })
    });

    view! {
        <section class="news-page">
            <div class="news-sources">
                {[(Source::HackerNews, "Hacker News"), (Source::Lobsters, "lobste.rs")]
                    .map(|(s, label)| {
                        view! {
                            <button
                                class:active=move || source.get() == s
                                on:click=move |_| source.set(s)
                            >
                                {label}
                            </button>
                        }
                    })}
            </div>
            <div class="posts-tabs">
                {[(0u8, "24h"), (1, "48h"), (2, "Week")]
                    .map(|(idx, label)| {
                        view! {
                            <button
                                class:active=move || range.get() == idx
                                on:click=move |_| range.set(idx)
                            >
                                {label}
                            </button>
                        }
                    })}
            </div>
            {move || match data.get() {
                None => view! { <p class="muted">"Loading…"</p> }.into_any(),
                Some(Err(err)) => view! { <p class="error">{err}</p> }.into_any(),
                Some(Ok(_)) => {
                    let client = client.clone();
                    let current = source.get();
                    view! {
                        <ul class="feed-list">
                            {move || {
                                let client = client.clone();
                                window
                                    .get()
                                    .map(|(items, anchor)| {
                                        items
                                            .into_iter()
                                            .map(|item| {
                                                post_row_view(item, anchor, current, client.clone())
                                            })
                                            .collect_view()
                                    })
                            }}
                        </ul>
                    }
                        .into_any()
                }
            }}
        </section>
    }
}
