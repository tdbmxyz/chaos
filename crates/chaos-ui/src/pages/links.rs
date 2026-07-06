use std::time::Duration;

use chaos_domain::{
    ArchiveState, Collection, CollectionRequest, CreateLinkRequest, Link, LinkQuery,
    UpdateLinkRequest,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use uuid::Uuid;

use crate::components::Modal;
use crate::use_client;

type CollectionsResource = LocalResource<chaos_client::Result<Vec<Collection>>>;

/// Links shown per page; filters reset paging to the first page.
const PAGE_SIZE: u32 = 50;

/// Links page: collection sidebar, tag filters, search, quick-add, link list
/// and the edit dialogs. All mutations bump `refresh` to re-run the resources.
#[component]
pub fn Links() -> impl IntoView {
    let refresh = RwSignal::new(0u32);
    let selected_collection = RwSignal::new(None::<Uuid>);
    let selected_tag = RwSignal::new(None::<String>);
    let search = RwSignal::new(String::new());
    let page_index = RwSignal::new(0u32);
    let editing_link = RwSignal::new(None::<Link>);
    let editing_collection = RwSignal::new(None::<Collection>);

    // Changing any filter jumps back to the first page.
    let filters = Memo::new(move |_| (selected_collection.get(), selected_tag.get(), search.get()));
    Effect::new(move |prev: Option<()>| {
        filters.track();
        if prev.is_some() {
            page_index.set(0);
        }
    });

    let client = use_client();
    let links = LocalResource::new({
        let client = client.clone();
        move || {
            refresh.track();
            let query = LinkQuery {
                collection_id: selected_collection.get(),
                tag: selected_tag.get(),
                q: Some(search.get()).filter(|q| !q.trim().is_empty()),
                limit: Some(PAGE_SIZE),
                offset: Some(page_index.get() * PAGE_SIZE),
            };
            let client = client.clone();
            async move { client.list_links(&query).await }
        }
    });
    let collections: CollectionsResource = LocalResource::new({
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
            <CollectionSidebar
                collections
                selected=selected_collection
                editing=editing_collection
                refresh
            />
            <section class="links-main">
                <div class="links-toolbar">
                    <input
                        type="search"
                        placeholder="Search links (includes archived page text)…"
                        prop:value=search
                        on:input=move |ev| search.set(event_target_value(&ev))
                    />
                </div>
                <TagFilter tags selected=selected_tag/>
                <AddLinkForm selected_collection refresh/>
                {move || match links.get() {
                    None => view! { <p class="muted">"Loading links…"</p> }.into_any(),
                    Some(Ok(page)) => {
                        // Light polling while snapshots are being taken, so
                        // "archiving…" badges resolve without manual refresh.
                        if page
                            .items
                            .iter()
                            .any(|l| matches!(l.archive, ArchiveState::Pending))
                        {
                            set_timeout(
                                move || refresh.update(|n| *n += 1),
                                Duration::from_secs(4),
                            );
                        }
                        let total = page.total;
                        let pages = total.div_ceil(PAGE_SIZE as u64).max(1) as u32;
                        let current = page_index;
                        view! {
                            <p class="muted links-count">
                                {total} " link" {if total == 1 { "" } else { "s" }}
                            </p>
                            <LinkList links=page.items editing=editing_link refresh/>
                            {(pages > 1)
                                .then(|| {
                                    view! {
                                        <div class="pager">
                                            <button
                                                disabled=move || current.get() == 0
                                                on:click=move |_| current.update(|p| *p = p.saturating_sub(1))
                                            >
                                                "‹ Prev"
                                            </button>
                                            <span class="muted">
                                                {move || format!("{} / {pages}", current.get() + 1)}
                                            </span>
                                            <button
                                                disabled=move || current.get() + 1 >= pages
                                                on:click=move |_| current.update(|p| *p += 1)
                                            >
                                                "Next ›"
                                            </button>
                                        </div>
                                    }
                                })}
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
        {move || {
            editing_link
                .get()
                .map(|link| view! { <EditLinkModal link collections editing=editing_link refresh/> })
        }}
        {move || {
            editing_collection
                .get()
                .map(|collection| {
                    view! {
                        <EditCollectionModal
                            collection
                            collections
                            editing=editing_collection
                            selected=selected_collection
                            refresh
                        />
                    }
                })
        }}
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

/// Options of a collection <select>, hierarchy shown by indentation.
/// `exclude` drops one collection (a collection cannot be its own parent).
#[component]
fn CollectionOptions(
    collections: CollectionsResource,
    current: Option<Uuid>,
    exclude: Option<Uuid>,
) -> impl IntoView {
    move || match collections.get() {
        Some(Ok(list)) => with_depth(&list)
            .into_iter()
            .filter(|(_, c)| Some(c.id) != exclude)
            .map(|(depth, c)| {
                let label = format!("{}{}", "\u{a0}\u{a0}".repeat(depth), c.name);
                view! {
                    <option value=c.id.to_string() selected=current == Some(c.id)>
                        {label}
                    </option>
                }
            })
            .collect_view()
            .into_any(),
        _ => ().into_any(),
    }
}

#[component]
fn CollectionSidebar(
    collections: CollectionsResource,
    selected: RwSignal<Option<Uuid>>,
    editing: RwSignal<Option<Collection>>,
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
                        let edit_target = c.clone();
                        view! {
                            <div class="collection-row">
                                <button
                                    class="collection-item"
                                    class:active=move || selected.get() == Some(id)
                                    style:padding-left=format!("{}rem", 0.75 + depth as f64 * 0.9)
                                    on:click=move |_| selected.set(Some(id))
                                >
                                    {c.name.clone()}
                                </button>
                                <button
                                    class="icon-btn"
                                    title="Edit collection"
                                    on:click=move |_| editing.set(Some(edit_target.clone()))
                                >
                                    "✎"
                                </button>
                            </div>
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

    let save = {
        let client = client.clone();
        move || {
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
                tags: split_tags(&tags.get_untracked()),
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
        }
    };

    // Shared into the app (Android share sheet → /links?add=<text>): pull
    // the first URL out of the payload and save it right away; anything
    // without a recognizable URL only prefills the field for manual fixing.
    let shared = leptos_router::hooks::use_query_map()
        .get_untracked()
        .get("add");
    if let Some(text) = shared {
        let first_url = text
            .split_whitespace()
            .find(|t| t.starts_with("http://") || t.starts_with("https://"))
            .map(str::to_string);
        match first_url {
            Some(u) => {
                url.set(u);
                save();
            }
            None => url.set(text),
        }
        let navigate = leptos_router::hooks::use_navigate();
        spawn_local(async move {
            navigate(
                "/links",
                leptos_router::NavigateOptions {
                    replace: true,
                    ..Default::default()
                },
            );
        });
    }

    let submit = {
        let save = save.clone();
        move |ev: leptos::ev::SubmitEvent| {
            ev.prevent_default();
            save();
        }
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
                placeholder="Title (fetched if empty)"
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

fn split_tags(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

#[component]
fn LinkList(
    links: Vec<Link>,
    editing: RwSignal<Option<Link>>,
    refresh: RwSignal<u32>,
) -> impl IntoView {
    if links.is_empty() {
        return view! { <p class="muted">"Nothing here yet — add your first link above."</p> }
            .into_any();
    }
    view! {
        <ul class="link-list">
            {links
                .into_iter()
                .map(|link| view! { <LinkItem link editing refresh/> })
                .collect_view()}
        </ul>
    }
    .into_any()
}

#[component]
fn LinkItem(link: Link, editing: RwSignal<Option<Link>>, refresh: RwSignal<u32>) -> impl IntoView {
    let client = use_client();
    let id = link.id;
    let host = link.url.host_str().unwrap_or_default().to_string();
    let edit_target = link.clone();

    let delete = {
        let client = client.clone();
        move |_| {
            let client = client.clone();
            spawn_local(async move {
                match client.delete_link(id).await {
                    Ok(()) => refresh.update(|n| *n += 1),
                    Err(err) => leptos::logging::warn!("delete link: {err}"),
                }
            });
        }
    };
    let rearchive = {
        let client = client.clone();
        move |_| {
            let client = client.clone();
            spawn_local(async move {
                match client.archive_link(id).await {
                    Ok(_) => refresh.update(|n| *n += 1),
                    Err(err) => leptos::logging::warn!("archive link: {err}"),
                }
            });
        }
    };

    let archive_badge = match &link.archive {
        ArchiveState::Archived { .. } => client
            .archive_view_url(id)
            .map(|url| {
                view! {
                    <a class="chip small archive-ok" href=url.to_string() target="_blank">
                        "archived"
                    </a>
                }
                .into_any()
            })
            .unwrap_or(().into_any()),
        ArchiveState::Pending => {
            view! { <span class="chip small archive-pending">"archiving…"</span> }.into_any()
        }
        ArchiveState::Failed { reason, .. } => {
            let reason = reason.clone();
            view! {
                <span class="chip small archive-failed" title=reason>
                    "archive failed"
                </span>
            }
            .into_any()
        }
        ArchiveState::None => ().into_any(),
    };

    view! {
        <li class="link-item">
            <div class="link-item-body">
                <a href=link.url.to_string() target="_blank" rel="noreferrer" class="link-title">
                    {(!host.is_empty())
                        .then(|| client.icon_url(&format!("fav:{host}")))
                        .flatten()
                        .map(|src| {
                            view! {
                                <img
                                    class="link-favicon"
                                    src=src.to_string()
                                    loading="lazy"
                                    onerror="this.style.display='none'"
                                />
                            }
                        })}
                    {link.title}
                </a>
                <span class="muted link-host">{host}</span>
                {link.description.map(|d| view! { <p class="link-desc">{d}</p> })}
                <div class="link-tags">
                    {archive_badge}
                    {link
                        .tags
                        .into_iter()
                        .map(|t| view! { <span class="chip small">{t.name}</span> })
                        .collect_view()}
                </div>
            </div>
            <div class="link-actions">
                <button class="icon-btn" title="Snapshot page" on:click=rearchive>
                    "↻"
                </button>
                <button
                    class="icon-btn"
                    title="Edit"
                    on:click=move |_| editing.set(Some(edit_target.clone()))
                >
                    "✎"
                </button>
                <button class="icon-btn danger" title="Delete" on:click=delete>
                    "✕"
                </button>
            </div>
        </li>
    }
}

#[component]
fn EditLinkModal(
    link: Link,
    collections: CollectionsResource,
    editing: RwSignal<Option<Link>>,
    refresh: RwSignal<u32>,
) -> impl IntoView {
    let client = use_client();
    let id = link.id;
    let url = RwSignal::new(link.url.to_string());
    let title = RwSignal::new(link.title.clone());
    let description = RwSignal::new(link.description.clone().unwrap_or_default());
    let tags = RwSignal::new(
        link.tags
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    );
    let collection = RwSignal::new(
        link.collection_id
            .map(|c| c.to_string())
            .unwrap_or_default(),
    );
    let error = RwSignal::new(None::<String>);

    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let parsed = match url.get_untracked().trim().parse::<url::Url>() {
            Ok(u) => u,
            Err(_) => {
                error.set(Some("invalid URL".into()));
                return;
            }
        };
        let req = UpdateLinkRequest {
            url: parsed,
            title: title.get_untracked().trim().to_string(),
            description: Some(description.get_untracked())
                .map(|d| d.trim().to_string())
                .filter(|d| !d.is_empty()),
            collection_id: Uuid::parse_str(&collection.get_untracked()).ok(),
            tags: split_tags(&tags.get_untracked()),
        };
        let client = client.clone();
        spawn_local(async move {
            match client.update_link(id, &req).await {
                Ok(_) => {
                    editing.set(None);
                    refresh.update(|n| *n += 1);
                }
                Err(err) => error.set(Some(err.to_string())),
            }
        });
    };

    view! {
        <Modal title="Edit link".to_string() on_close=move |_: ()| editing.set(None)>
            <form class="modal-form" on:submit=save>
                <label>
                    "URL"
                    <input
                        type="text"
                        prop:value=url
                        on:input=move |ev| url.set(event_target_value(&ev))
                    />
                </label>
                <label>
                    "Title"
                    <input
                        type="text"
                        prop:value=title
                        on:input=move |ev| title.set(event_target_value(&ev))
                    />
                </label>
                <label>
                    "Description"
                    <textarea
                        prop:value=description
                        on:input=move |ev| description.set(event_target_value(&ev))
                    ></textarea>
                </label>
                <label>
                    "Collection"
                    <select on:change=move |ev| collection.set(event_target_value(&ev))>
                        <option value="" selected=link.collection_id.is_none()>
                            "(unsorted)"
                        </option>
                        <CollectionOptions collections current=link.collection_id exclude=None/>
                    </select>
                </label>
                <label>
                    "Tags (comma separated)"
                    <input
                        type="text"
                        prop:value=tags
                        on:input=move |ev| tags.set(event_target_value(&ev))
                    />
                </label>
                {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
                <div class="modal-actions">
                    <button type="button" on:click=move |_| editing.set(None)>
                        "Cancel"
                    </button>
                    <button type="submit" class="primary">
                        "Save"
                    </button>
                </div>
            </form>
        </Modal>
    }
}

#[component]
fn EditCollectionModal(
    collection: Collection,
    collections: CollectionsResource,
    editing: RwSignal<Option<Collection>>,
    selected: RwSignal<Option<Uuid>>,
    refresh: RwSignal<u32>,
) -> impl IntoView {
    let client = use_client();
    let id = collection.id;
    let name = RwSignal::new(collection.name.clone());
    let description = RwSignal::new(collection.description.clone().unwrap_or_default());
    let color = RwSignal::new(collection.color.clone().unwrap_or_default());
    let parent = RwSignal::new(
        collection
            .parent_id
            .map(|p| p.to_string())
            .unwrap_or_default(),
    );
    let error = RwSignal::new(None::<String>);
    let confirm_delete = RwSignal::new(false);

    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let req = CollectionRequest {
            name: name.get_untracked().trim().to_string(),
            description: Some(description.get_untracked())
                .map(|d| d.trim().to_string())
                .filter(|d| !d.is_empty()),
            color: Some(color.get_untracked())
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty()),
            parent_id: Uuid::parse_str(&parent.get_untracked()).ok(),
        };
        let client = client.clone();
        spawn_local(async move {
            match client.update_collection(id, &req).await {
                Ok(_) => {
                    editing.set(None);
                    refresh.update(|n| *n += 1);
                }
                Err(err) => error.set(Some(err.to_string())),
            }
        });
    };

    let client_del = use_client();
    let delete = move |_| {
        if !confirm_delete.get_untracked() {
            confirm_delete.set(true);
            return;
        }
        let client = client_del.clone();
        spawn_local(async move {
            match client.delete_collection(id).await {
                Ok(()) => {
                    if selected.get_untracked() == Some(id) {
                        selected.set(None);
                    }
                    editing.set(None);
                    refresh.update(|n| *n += 1);
                }
                Err(err) => error.set(Some(err.to_string())),
            }
        });
    };

    view! {
        <Modal title="Edit collection".to_string() on_close=move |_: ()| editing.set(None)>
            <form class="modal-form" on:submit=save>
                <label>
                    "Name"
                    <input
                        type="text"
                        prop:value=name
                        on:input=move |ev| name.set(event_target_value(&ev))
                    />
                </label>
                <label>
                    "Description"
                    <textarea
                        prop:value=description
                        on:input=move |ev| description.set(event_target_value(&ev))
                    ></textarea>
                </label>
                <label>
                    "Color (hex, e.g. #7c9aff)"
                    <input
                        type="text"
                        prop:value=color
                        on:input=move |ev| color.set(event_target_value(&ev))
                    />
                </label>
                <label>
                    "Parent collection"
                    <select on:change=move |ev| parent.set(event_target_value(&ev))>
                        <option value="" selected=collection.parent_id.is_none()>
                            "(root)"
                        </option>
                        <CollectionOptions
                            collections
                            current=collection.parent_id
                            exclude=Some(id)
                        />
                    </select>
                </label>
                {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
                <div class="modal-actions">
                    <button type="button" class="danger" on:click=delete>
                        {move || {
                            if confirm_delete.get() {
                                "Really delete? Links become unsorted"
                            } else {
                                "Delete collection"
                            }
                        }}
                    </button>
                    <span class="grow"></span>
                    <button type="button" on:click=move |_| editing.set(None)>
                        "Cancel"
                    </button>
                    <button type="submit" class="primary">
                        "Save"
                    </button>
                </div>
            </form>
        </Modal>
    }
}
