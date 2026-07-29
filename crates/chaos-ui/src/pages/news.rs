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

    // Kept in `NewsCache` (app-level context) rather than a per-mount
    // resource, so coming back to this tab renders the list already loaded
    // instead of re-fetching and flashing "Loading…".
    let cache = crate::use_news_cache();
    let error = RwSignal::new(None::<String>);

    // One fetch, replacing whatever the cache holds. Used on first load, on a
    // source switch, on reconnect, and by the pull gesture.
    let reload = {
        let client = client.clone();
        move || {
            let client = client.clone();
            let wanted = source.get_untracked();
            async move {
                match load_posts(wanted, conn, &client).await {
                    Ok((posts, more)) => {
                        error.set(None);
                        cache.loaded.set(Some((wanted, posts, more)));
                    }
                    // Keep showing what we have: a failed refresh should not
                    // empty a list the user is reading.
                    Err(err) => error.set(Some(err)),
                }
            }
        }
    };

    // Fetch only when the cache can't answer — nothing loaded yet, or it holds
    // a different source — or when connectivity just came back mid-visit.
    // Being *currently* online is deliberately not a reason: that would fetch
    // on every mount, which is the behaviour this cache exists to remove.
    let was_online = StoredValue::new(false);
    Effect::new({
        let reload = reload.clone();
        move |prev: Option<()>| {
            let wanted = source.get();
            let online = conn.get() == crate::offline::Connectivity::Online;
            // Not on the first run: `was_online` starts false on every mount,
            // so every visit would otherwise look like a reconnection.
            let came_online = online && !was_online.get_value() && prev.is_some();
            was_online.set_value(online);
            let stale =
                !matches!(cache.loaded.get_untracked(), Some((cached, _, _)) if cached == wanted);
            if stale || came_online {
                let reload = reload.clone();
                spawn_local(async move { reload().await });
            }
        }
    });

    let pull = crate::hooks::use_pull_to_refresh(move || {
        let reload = reload.clone();
        async move { reload().await }
    });

    // Restore where the user was, and keep it current as they scroll.
    let scroll_listener = window_event_listener(leptos::ev::scroll, move |_| {
        if let Some(y) = web_sys::window().and_then(|w| w.scroll_y().ok()) {
            cache.scroll.set(y);
        }
    });
    on_cleanup(move || scroll_listener.remove());
    Effect::new(move |prev: Option<()>| {
        if prev.is_some() {
            return;
        }
        let y = cache.scroll.get_untracked();
        if y > 0.0
            && let Some(window) = web_sys::window()
        {
            // After the rows commit, or there is nothing to scroll through yet.
            set_timeout(
                move || window.scroll_to_with_x_and_y(0.0, y),
                std::time::Duration::from_millis(0),
            );
        }
    });

    // The visible list as ONE top-level reactive closure: it reads `data`,
    // `range`, and `source`, so a range click re-runs it (re-reading the
    // already-loaded payload — no refetch) and swaps the window. Kept flat
    // (no nested reactive block) so the range subscription is unmistakable.
    // The union anchor spans all three windows, so colors never rescale.
    let list = {
        let client = client.clone();
        move || match (cache.loaded.get(), error.get()) {
            // Nothing cached and the fetch failed: the error is all we have.
            (None, Some(err)) => view! { <p class="error">{err}</p> }.into_any(),
            (None, None) => view! { <p class="muted">"Loading…"</p> }.into_any(),
            (Some((_, posts, _)), _) => {
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
            cache.loaded.track();
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
            // Follows the finger while pulling, then spins until the refresh
            // resolves. Touch-only, so it never appears on a desktop browser.
            <div
                class="pull-refresh"
                class:spinning=move || pull.refreshing.get()
                class:armed=move || pull.armed()
                style:height=move || {
                    if pull.refreshing.get() {
                        "36px".to_string()
                    } else {
                        format!("{}px", pull.distance.get().min(72.0))
                    }
                }
            >
                <span class="pull-spinner"></span>
            </div>
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
