use std::cell::RefCell;
use std::rc::Rc;

use chaos_domain::{Source, ViewEvent};
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::JsValue;

use super::dashboard::{PostsTab, load_posts, post_row_view, posts_window, score_anchor};
use crate::analytics::{self, ViewedState};
use crate::use_client;

/// The IntersectionObserver callback + observer, kept alive for as long as the
/// page is mounted (dropping the `Closure` would invalidate the JS callback).
type ObserverCell = Rc<RefCell<Option<(web_sys::IntersectionObserver, ObserverClosure)>>>;
type ObserverClosure = Closure<dyn FnMut(js_sys::Array, web_sys::IntersectionObserver)>;

/// (Re)bind an IntersectionObserver over every currently rendered
/// `li.post-row[data-view-id]`, marking a row `Seen` once it is at least half
/// in view. The previous observer (if any) is disconnected first. Browser-only.
fn rebind_seen_observer(cell: &ObserverCell) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    // Drop the previous observer/closure before building the new one.
    if let Some((old, _)) = cell.borrow_mut().take() {
        old.disconnect();
    }

    let cb = Closure::<dyn FnMut(js_sys::Array, web_sys::IntersectionObserver)>::new(
        move |entries: js_sys::Array, _obs: web_sys::IntersectionObserver| {
            entries.for_each(&mut |entry, _, _| {
                let Ok(entry) = entry.dyn_into::<web_sys::IntersectionObserverEntry>() else {
                    return;
                };
                if entry.intersection_ratio() < 0.5 {
                    return;
                }
                let Some(vid) = entry
                    .target()
                    .dyn_ref::<web_sys::Element>()
                    .and_then(|el| el.get_attribute("data-view-id"))
                else {
                    return;
                };
                if let Some((src, id)) = vid.split_once(':')
                    && let Some(source) = Source::from_str(src)
                {
                    analytics::record_view(source, id, ViewEvent::Seen);
                }
            });
        },
    );

    let init = web_sys::IntersectionObserverInit::new();
    init.set_threshold(&JsValue::from_f64(0.5));
    let Ok(observer) =
        web_sys::IntersectionObserver::new_with_options(cb.as_ref().unchecked_ref(), &init)
    else {
        return;
    };

    if let Ok(nodes) = document.query_selector_all("li.post-row[data-view-id]") {
        for i in 0..nodes.length() {
            if let Some(el) = nodes
                .get(i)
                .and_then(|n| n.dyn_into::<web_sys::Element>().ok())
            {
                observer.observe(&el);
            }
        }
    }
    *cell.borrow_mut() = Some((observer, cb));
}

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

    // The visible list as ONE top-level reactive closure: it reads `data`,
    // `range`, and `source`, so a range click re-runs it (re-reading the
    // already-loaded payload — no refetch) and swaps the window. Kept flat
    // (no nested reactive block) so the range subscription is unmistakable.
    // The union anchor spans all three windows, so colors never rescale.
    let list = {
        let client = client.clone();
        move || match data.get() {
            None => view! { <p class="muted">"Loading…"</p> }.into_any(),
            Some(Err(err)) => view! { <p class="error">{err}</p> }.into_any(),
            Some(Ok((posts, _))) => {
                let anchor = score_anchor(
                    posts
                        .last_24h
                        .iter()
                        .chain(&posts.last_48h)
                        .chain(&posts.last_week)
                        .map(|i| i.score),
                );
                let items = posts_window(&posts, range_tab(range.get()));
                if items.is_empty() {
                    return view! { <p class="muted">"Nothing in this window yet."</p> }.into_any();
                }
                let current = source.get();
                let client = client.clone();
                view! {
                    <ul class="feed-list">
                        {items
                            .into_iter()
                            .map(|item| post_row_view(item, anchor, current, client.clone()))
                            .collect_view()}
                    </ul>
                }
                .into_any()
            }
        }
    };

    // Viewed-state tracking is authed-only. When signed in: expose `ViewedState`
    // (so `post_row_view` renders + records), load the server viewed-map into
    // the overlay per source, and observe rows for the `Seen` signal.
    let authed = crate::use_session().0.get_untracked().is_some();
    if authed {
        provide_context(ViewedState {
            source: source.get_untracked(),
        });

        // Load the server viewed-map into the overlay whenever the source
        // changes (and on reconnect). Best-effort: offline/auth errors are
        // ignored — the overlay keeps whatever it has.
        Effect::new({
            let client = client.clone();
            move |_| {
                let src = source.get();
                conn.track();
                let client = client.clone();
                spawn_local(async move {
                    if let Ok(map) = client.viewed_map(src).await {
                        analytics::merge_server_map(src, map);
                    }
                });
            }
        });

        // Rebind the seen-observer after each list render (source/range/data
        // change swaps the row nodes).
        // The cell (and thus the live observer + callback) is owned by the
        // effect; when the page unmounts the effect is disposed, dropping the
        // cell and invalidating the JS callback.
        let observer: ObserverCell = Rc::new(RefCell::new(None));
        Effect::new(move |_| {
            // Track what rebuilds the row list so the observer re-binds.
            source.get();
            range.get();
            data.track();
            // Defer to the next tick: this effect fires when the data resolves,
            // but Leptos hasn't committed the new row `<li>`s to the DOM yet, so
            // querying for them here would observe nothing. A 0ms timeout runs
            // after the render commits.
            let observer = observer.clone();
            set_timeout(
                move || rebind_seen_observer(&observer),
                std::time::Duration::from_millis(0),
            );
        });
    }

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
            {list}
        </section>
    }
}
