use std::time::Duration;

use chaos_domain::BookmarkGroup;
use leptos::prelude::*;

use crate::components::ServiceGrid;
use crate::use_client;

const REFRESH_INTERVAL: Duration = Duration::from_secs(30);

#[component]
pub fn Dashboard() -> impl IntoView {
    let client = use_client();

    // Re-poll service statuses while the dashboard stays open.
    let tick = RwSignal::new(0u32);
    if let Ok(handle) = set_interval_with_handle(move || tick.update(|n| *n += 1), REFRESH_INTERVAL)
    {
        on_cleanup(move || handle.clear());
    }

    let services = LocalResource::new({
        let client = client.clone();
        move || {
            tick.track();
            let client = client.clone();
            async move { client.services().await }
        }
    });
    let dashboard = LocalResource::new({
        let client = client.clone();
        move || {
            let client = client.clone();
            async move { client.dashboard().await }
        }
    });

    view! {
        {move || {
            dashboard
                .get()
                .and_then(|d| d.ok())
                .and_then(|d| d.search_url)
                .map(|url| view! { <SearchBar template=url/> })
        }}
        <section>
            <h2>"Services"</h2>
            {move || match services.get() {
                None => view! { <p class="muted">"Checking services…"</p> }.into_any(),
                Some(Ok(list)) => view! { <ServiceGrid services=list/> }.into_any(),
                Some(Err(err)) => {
                    view! { <p class="error">"Could not reach chaos server: " {err.to_string()}</p> }
                        .into_any()
                }
            }}
        </section>
        {move || {
            dashboard
                .get()
                .and_then(|d| d.ok())
                .map(|d| d.bookmarks)
                .filter(|groups| !groups.is_empty())
                .map(|groups| view! { <Bookmarks groups/> })
        }}
    }
}

/// Search box opening the configured engine in a new tab.
#[component]
fn SearchBar(template: String) -> impl IntoView {
    let query = RwSignal::new(String::new());

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let q = query.get_untracked();
        let q = q.trim();
        if q.is_empty() {
            return;
        }
        let encoded: String = url::form_urlencoded::byte_serialize(q.as_bytes()).collect();
        let target = template.replace("{}", &encoded);
        let _ = window().open_with_url_and_target(&target, "_blank");
        query.set(String::new());
    };

    view! {
        <form class="dashboard-search" on:submit=submit>
            <input
                type="search"
                placeholder="Search the web…"
                autofocus
                prop:value=query
                on:input=move |ev| query.set(event_target_value(&ev))
            />
        </form>
    }
}

#[component]
fn Bookmarks(groups: Vec<BookmarkGroup>) -> impl IntoView {
    let client = use_client();

    view! {
        <section>
            <h2>"Bookmarks"</h2>
            <div class="bookmark-groups">
                {groups
                    .into_iter()
                    .map(|group| {
                        let client = client.clone();
                        view! {
                            <div class="bookmark-group">
                                <h3>{group.title}</h3>
                                <ul>
                                    {group
                                        .links
                                        .into_iter()
                                        .map(|bookmark| {
                                            let icon = bookmark
                                                .icon
                                                .as_deref()
                                                .and_then(|spec| client.icon_url(spec));
                                            view! {
                                                <li>
                                                    <a
                                                        href=bookmark.url.to_string()
                                                        target="_blank"
                                                        rel="noreferrer"
                                                    >
                                                        {icon
                                                            .map(|url| {
                                                                view! {
                                                                    <img
                                                                        class="bookmark-icon"
                                                                        src=url.to_string()
                                                                        loading="lazy"
                                                                        alt=""
                                                                    />
                                                                }
                                                            })}
                                                        {bookmark.title}
                                                    </a>
                                                </li>
                                            }
                                        })
                                        .collect_view()}
                                </ul>
                            </div>
                        }
                    })
                    .collect_view()}
            </div>
        </section>
    }
}
