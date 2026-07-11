//! Global quick-search overlay (Ctrl-K / Cmd-K): debounced input, grouped
//! results, arrow-key navigation. Mounted once at App level; opened from
//! the keyboard shortcut or the topbar/tabbar search buttons.

use std::time::Duration;

use chaos_domain::{SearchHit, SearchKind, SearchResults};
use leptos::prelude::*;

use crate::hooks::debounce_signal;
use crate::use_client;

/// Order-preserving flattening of the grouped results: index N in the
/// flat list is the Nth rendered row, so one cursor spans all groups.
fn flatten(results: &SearchResults) -> Vec<SearchHit> {
    results
        .services
        .iter()
        .chain(&results.bookmarks)
        .chain(&results.links)
        .chain(&results.events)
        .cloned()
        .collect()
}

/// Move the selection cursor by `dir` (±1), wrapping at both ends.
fn step(current: usize, len: usize, dir: isize) -> usize {
    if len == 0 {
        return 0;
    }
    (current.min(len - 1) as isize + dir).rem_euclid(len as isize) as usize
}

#[component]
pub fn QuickSearch(open: RwSignal<bool>) -> impl IntoView {
    let query = RwSignal::new(String::new());
    let selected = RwSignal::new(0usize);
    let input_ref = NodeRef::<leptos::html::Input>::new();

    // ~250ms debounce: forward the query only once it stopped changing.
    // Reuses the same trailing-debounce helper as the dashboard search box
    // instead of hand-rolling set_timeout bookkeeping here.
    let debounced = debounce_signal(query, Duration::from_millis(250));

    let client = use_client();
    let results = LocalResource::new(move || {
        let q = debounced.get();
        let client = client.clone();
        async move {
            if q.trim().is_empty() {
                return Ok(SearchResults::default());
            }
            client.search(&q).await
        }
    });

    // New results reset the cursor; opening focuses the input.
    Effect::new(move |_| {
        debounced.track();
        selected.set(0);
    });
    Effect::new(move |_| {
        if open.get()
            && let Some(input) = input_ref.get()
        {
            let _ = input.focus();
        }
    });

    let close = move || {
        open.set(false);
        query.set(String::new());
        selected.set(0);
    };

    let navigate = leptos_router::hooks::use_navigate();
    let activate = Callback::new(move |hit: SearchHit| {
        match hit.kind {
            // Events carry no URL of their own; land on the calendar.
            SearchKind::Event => navigate("/calendar", Default::default()),
            _ => {
                if let Some(url) = &hit.url
                    && !crate::open_external(url.as_str())
                    && let Some(window) = web_sys::window()
                {
                    let _ = window.open_with_url_and_target(url.as_str(), "_blank");
                }
            }
        }
        close();
    });

    let keydown = move |ev: leptos::ev::KeyboardEvent| {
        let flat = results
            .get_untracked()
            .and_then(|r| r.ok())
            .map(|r| flatten(&r))
            .unwrap_or_default();
        match ev.key().as_str() {
            "ArrowDown" => {
                ev.prevent_default();
                selected.update(|s| *s = step(*s, flat.len(), 1));
            }
            "ArrowUp" => {
                ev.prevent_default();
                selected.update(|s| *s = step(*s, flat.len(), -1));
            }
            "Enter" => {
                if let Some(hit) = flat.into_iter().nth(selected.get_untracked()) {
                    activate.run(hit);
                }
            }
            "Escape" => {
                // Don't let the window-level Modal listener see this press:
                // Ctrl-K over an open dialog must not discard the dialog.
                ev.stop_propagation();
                close();
            }
            _ => {}
        }
    };

    view! {
        {move || {
            open.get()
                .then(|| {
                    view! {
                        <div class="quick-search-backdrop" on:click=move |_| close()>
                            <div class="quick-search" on:click=|ev| ev.stop_propagation()>
                                <input
                                    class="quick-search-input"
                                    type="search"
                                    placeholder="Search services, bookmarks, links, events…"
                                    prop:value=query
                                    on:input=move |ev| query.set(event_target_value(&ev))
                                    on:keydown=keydown
                                    node_ref=input_ref
                                />
                                <div class="quick-search-results">
                                    {move || match results.get() {
                                        None => view! { <p class="muted">"Searching…"</p> }.into_any(),
                                        Some(Err(err)) => {
                                            view! { <p class="muted">{format!("Search failed: {err}")}</p> }
                                                .into_any()
                                        }
                                        Some(Ok(res)) => {
                                            if flatten(&res).is_empty() {
                                                let label = if query.get_untracked().trim().is_empty() {
                                                    "Type to search"
                                                } else {
                                                    "No results"
                                                };
                                                return view! { <p class="muted">{label}</p> }.into_any();
                                            }
                                            let mut index = 0usize;
                                            [
                                                ("Services", res.services),
                                                ("Bookmarks", res.bookmarks),
                                                ("Links", res.links),
                                                ("Events", res.events),
                                            ]
                                                .into_iter()
                                                .filter(|(_, hits)| !hits.is_empty())
                                                .map(|(label, hits)| {
                                                    view! {
                                                        <div class="qs-group">
                                                            <h4 class="muted">{label}</h4>
                                                            {hits
                                                                .into_iter()
                                                                .map(|hit| {
                                                                    let i = index;
                                                                    index += 1;
                                                                    let title = hit.title.clone();
                                                                    let subtitle =
                                                                        hit.subtitle.clone().unwrap_or_default();
                                                                    view! {
                                                                        <button
                                                                            class="qs-row"
                                                                            class:selected=move || selected.get() == i
                                                                            on:click=move |_| activate.run(hit.clone())
                                                                        >
                                                                            <span class="qs-title">{title}</span>
                                                                            <span class="qs-sub muted">{subtitle}</span>
                                                                        </button>
                                                                    }
                                                                })
                                                                .collect_view()}
                                                        </div>
                                                    }
                                                })
                                                .collect_view()
                                                .into_any()
                                        }
                                    }}
                                </div>
                            </div>
                        </div>
                    }
                })
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(kind: SearchKind, title: &str) -> SearchHit {
        SearchHit {
            kind,
            title: title.into(),
            subtitle: None,
            url: None,
        }
    }

    #[test]
    fn flatten_preserves_group_order_for_keyboard_navigation() {
        let results = SearchResults {
            services: vec![hit(SearchKind::Service, "Jellyfin")],
            bookmarks: vec![hit(SearchKind::Bookmark, "GitHub")],
            links: vec![
                hit(SearchKind::Link, "Rust blog"),
                hit(SearchKind::Link, "Leptos"),
            ],
            events: vec![hit(SearchKind::Event, "Dentist")],
        };
        let titles: Vec<_> = flatten(&results).iter().map(|h| h.title.clone()).collect();
        assert_eq!(
            titles,
            ["Jellyfin", "GitHub", "Rust blog", "Leptos", "Dentist"]
        );
        assert!(flatten(&SearchResults::default()).is_empty());
    }

    #[test]
    fn step_wraps_in_both_directions_and_survives_empty_lists() {
        assert_eq!(step(0, 3, 1), 1);
        assert_eq!(step(2, 3, 1), 0);
        assert_eq!(step(0, 3, -1), 2);
        assert_eq!(step(0, 0, 1), 0);
        // Cursor beyond the list (results shrank) clamps before stepping.
        assert_eq!(step(9, 3, 1), 0);
    }
}
