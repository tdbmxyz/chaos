use chaos_domain::{Collection, CollectionRequest, CreateLinkRequest, Link, LinkQuery};
use leptos::prelude::*;
use leptos::task::spawn_local;
use uuid::Uuid;

use crate::use_client;

/// Links page: collection sidebar, tag filters, search, quick-add and the
/// link list itself. All mutations bump `refresh` to re-run the resources.
#[component]
pub fn Links() -> impl IntoView {
    let refresh = RwSignal::new(0u32);
    let selected_collection = RwSignal::new(None::<Uuid>);
    let selected_tag = RwSignal::new(None::<String>);
    let search = RwSignal::new(String::new());

    let client = use_client();
    let links = LocalResource::new({
        let client = client.clone();
        move || {
            refresh.track();
            let query = LinkQuery {
                collection_id: selected_collection.get(),
                tag: selected_tag.get(),
                q: Some(search.get()).filter(|q| !q.trim().is_empty()),
                ..Default::default()
            };
            let client = client.clone();
            async move { client.list_links(&query).await }
        }
    });
    let collections = LocalResource::new({
        let client = client.clone();
        move || {
            refresh.track();
            let client = client.clone();
            async move { client.list_collections().await }
        }
    });
    let tags = LocalResource::new({
        let client = client.clone();
        move || {
            refresh.track();
            let client = client.clone();
            async move { client.list_tags().await }
        }
    });

    view! {
        <div class="links-layout">
            <CollectionSidebar collections selected=selected_collection refresh/>
            <section class="links-main">
                <div class="links-toolbar">
                    <input
                        type="search"
                        placeholder="Search links…"
                        prop:value=search
                        on:input=move |ev| search.set(event_target_value(&ev))
                    />
                </div>
                <TagFilter tags selected=selected_tag/>
                <AddLinkForm selected_collection refresh/>
                {move || match links.get() {
                    None => view! { <p class="muted">"Loading links…"</p> }.into_any(),
                    Some(Ok(page)) => {
                        let total = page.total;
                        view! {
                            <p class="muted links-count">{total} " link" {if total == 1 { "" } else { "s" }}</p>
                            <LinkList links=page.items refresh/>
                        }
                            .into_any()
                    }
                    Some(Err(err)) => {
                        view! { <p class="error">"Failed to load links: " {err.to_string()}</p> }
                            .into_any()
                    }
                }}
            </section>
        </div>
    }
}

/// Flatten the collection tree into (depth, collection) for indented display.
/// Collections whose parent is missing are treated as roots.
fn with_depth(collections: &[Collection]) -> Vec<(usize, Collection)> {
    fn walk(all: &[Collection], parent: Uuid, depth: usize, out: &mut Vec<(usize, Collection)>) {
        for c in all.iter().filter(|c| c.parent_id == Some(parent)) {
            out.push((depth, c.clone()));
            walk(all, c.id, depth + 1, out);
        }
    }

    let ids: Vec<Uuid> = collections.iter().map(|c| c.id).collect();
    let mut out = Vec::new();
    for c in collections
        .iter()
        .filter(|c| c.parent_id.is_none_or(|p| !ids.contains(&p)))
    {
        out.push((0, c.clone()));
        walk(collections, c.id, 1, &mut out);
    }
    out
}

#[component]
fn CollectionSidebar(
    collections: LocalResource<chaos_client::Result<Vec<Collection>>>,
    selected: RwSignal<Option<Uuid>>,
    refresh: RwSignal<u32>,
) -> impl IntoView {
    let client = use_client();
    let new_name = RwSignal::new(String::new());

    let create = move |_| {
        let name = new_name.get_untracked().trim().to_string();
        if name.is_empty() {
            return;
        }
        let client = client.clone();
        spawn_local(async move {
            let req = CollectionRequest {
                name,
                description: None,
                color: None,
                parent_id: selected.get_untracked(),
            };
            match client.create_collection(&req).await {
                Ok(_) => {
                    new_name.set(String::new());
                    refresh.update(|n| *n += 1);
                }
                Err(err) => leptos::logging::warn!("create collection: {err}"),
            }
        });
    };

    view! {
        <aside class="collections">
            <h3>"Collections"</h3>
            <button
                class="collection-item"
                class:active=move || selected.get().is_none()
                on:click=move |_| selected.set(None)
            >
                "All links"
            </button>
            {move || match collections.get() {
                Some(Ok(list)) => with_depth(&list)
                    .into_iter()
                    .map(|(depth, c)| {
                        let id = c.id;
                        view! {
                            <button
                                class="collection-item"
                                class:active=move || selected.get() == Some(id)
                                style:padding-left=format!("{}rem", 0.75 + depth as f64 * 0.9)
                                on:click=move |_| selected.set(Some(id))
                            >
                                {c.name.clone()}
                            </button>
                        }
                    })
                    .collect_view()
                    .into_any(),
                _ => ().into_any(),
            }}
            <div class="collection-add">
                <input
                    type="text"
                    placeholder="New collection (under selection)"
                    prop:value=new_name
                    on:input=move |ev| new_name.set(event_target_value(&ev))
                />
                <button on:click=create>"+"</button>
            </div>
        </aside>
    }
}

#[component]
fn TagFilter(
    tags: LocalResource<chaos_client::Result<Vec<chaos_domain::TagWithCount>>>,
    selected: RwSignal<Option<String>>,
) -> impl IntoView {
    view! {
        <div class="tag-filter">
            {move || match tags.get() {
                Some(Ok(list)) if !list.is_empty() => list
                    .into_iter()
                    .map(|t| {
                        let name = t.tag.name.clone();
                        let is_active = {
                            let name = name.clone();
                            move || selected.get().as_deref() == Some(name.as_str())
                        };
                        let toggle = {
                            let name = name.clone();
                            move |_| {
                                selected.update(|s| {
                                    *s = if s.as_deref() == Some(name.as_str()) {
                                        None
                                    } else {
                                        Some(name.clone())
                                    }
                                })
                            }
                        };
                        view! {
                            <button class="chip" class:active=is_active on:click=toggle>
                                {t.tag.name} <span class="chip-count">{t.link_count}</span>
                            </button>
                        }
                    })
                    .collect_view()
                    .into_any(),
                _ => ().into_any(),
            }}
        </div>
    }
}

#[component]
fn AddLinkForm(
    selected_collection: RwSignal<Option<Uuid>>,
    refresh: RwSignal<u32>,
) -> impl IntoView {
    let client = use_client();
    let url = RwSignal::new(String::new());
    let title = RwSignal::new(String::new());
    let tags = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let parsed = match url.get_untracked().trim().parse::<url::Url>() {
            Ok(u) => u,
            Err(_) => {
                error.set(Some("invalid URL".into()));
                return;
            }
        };
        let req = CreateLinkRequest {
            url: parsed,
            title: Some(title.get_untracked())
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty()),
            description: None,
            collection_id: selected_collection.get_untracked(),
            tags: tags
                .get_untracked()
                .split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect(),
        };
        let client = client.clone();
        spawn_local(async move {
            match client.create_link(&req).await {
                Ok(_) => {
                    url.set(String::new());
                    title.set(String::new());
                    tags.set(String::new());
                    error.set(None);
                    refresh.update(|n| *n += 1);
                }
                Err(err) => error.set(Some(err.to_string())),
            }
        });
    };

    view! {
        <form class="add-link" on:submit=submit>
            <input
                type="text"
                class="grow"
                placeholder="https://… (saved into the selected collection)"
                prop:value=url
                on:input=move |ev| url.set(event_target_value(&ev))
            />
            <input
                type="text"
                placeholder="Title (optional)"
                prop:value=title
                on:input=move |ev| title.set(event_target_value(&ev))
            />
            <input
                type="text"
                placeholder="tags, comma, separated"
                prop:value=tags
                on:input=move |ev| tags.set(event_target_value(&ev))
            />
            <button type="submit">"Add"</button>
            {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
        </form>
    }
}

#[component]
fn LinkList(links: Vec<Link>, refresh: RwSignal<u32>) -> impl IntoView {
    if links.is_empty() {
        return view! { <p class="muted">"Nothing here yet — add your first link above."</p> }
            .into_any();
    }
    view! {
        <ul class="link-list">
            {links
                .into_iter()
                .map(|link| view! { <LinkItem link refresh/> })
                .collect_view()}
        </ul>
    }
    .into_any()
}

#[component]
fn LinkItem(link: Link, refresh: RwSignal<u32>) -> impl IntoView {
    let client = use_client();
    let id = link.id;
    let host = link.url.host_str().unwrap_or_default().to_string();

    let delete = move |_| {
        let client = client.clone();
        spawn_local(async move {
            match client.delete_link(id).await {
                Ok(()) => refresh.update(|n| *n += 1),
                Err(err) => leptos::logging::warn!("delete link: {err}"),
            }
        });
    };

    view! {
        <li class="link-item">
            <div class="link-item-body">
                <a href=link.url.to_string() target="_blank" rel="noreferrer" class="link-title">
                    {link.title}
                </a>
                <span class="muted link-host">{host}</span>
                {link.description.map(|d| view! { <p class="link-desc">{d}</p> })}
                <div class="link-tags">
                    {link
                        .tags
                        .into_iter()
                        .map(|t| view! { <span class="chip small">{t.name}</span> })
                        .collect_view()}
                </div>
            </div>
            <button class="danger" title="Delete" on:click=delete>
                "✕"
            </button>
        </li>
    }
}
