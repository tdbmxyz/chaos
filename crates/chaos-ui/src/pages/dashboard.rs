use std::time::Duration;

use chaos_client::ClientError;
use chaos_domain::{
    BookmarkGroup, ColumnSize, DashboardLayout, FeedItem, HealthState, PostsData, ServerStats,
    SystemdAction, SystemdActionRequest, SystemdUnitStatus, WeatherData, Widget, WidgetData,
    WidgetInstance,
};
use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use leptos::prelude::*;

use crate::components::ServiceGrid;
use crate::use_client;

const SERVICES_REFRESH: Duration = Duration::from_secs(30);
/// Data widgets are cached server-side; this only controls how often an open
/// dashboard picks the fresh cache up.
const WIDGET_REFRESH: Duration = Duration::from_secs(300);

/// Busy flag + action dispatcher backing the systemd control buttons.
type SystemdControls = (RwSignal<bool>, Callback<(String, SystemdAction)>);

#[component]
pub fn Dashboard() -> impl IntoView {
    let client = use_client();
    let refresh = RwSignal::new(0u32);
    provide_context(crate::hooks::RefreshTick(refresh));

    // Cache-first so a booted-offline app still gets its layout; the stale
    // flag is dropped — the offline badge already tells the user, and the
    // layout has no per-widget staleness UI of its own.
    let conn = crate::offline::use_connectivity();
    let layout = LocalResource::new({
        let client = client.clone();
        move || {
            conn.track(); // recovery re-fetches the layout once
            let client = client.clone();
            async move {
                crate::offline::cached(conn, "dashboard", client.dashboard())
                    .await
                    .map(|(layout, _)| layout)
            }
        }
    });

    view! {
        <div class="dashboard-head">
            <button
                class="refresh-btn"
                title="Refresh all widgets"
                on:click=move |_| refresh.update(|n| *n += 1)
            >
                "↻"
            </button>
        </div>
        {move || match layout.get() {
            None => view! { <p class="muted">"Loading dashboard…"</p> }.into_any(),
            Some(Ok(layout)) => view! { <Columns layout/> }.into_any(),
            Some(Err(err)) => {
                view! { <p class="error">"Could not reach chaos server: " {err.to_string()}</p> }
                    .into_any()
            }
        }}
    }
}

#[component]
fn Columns(layout: DashboardLayout) -> impl IntoView {
    view! {
        <div class="dashboard-columns">
            {layout
                .columns
                .into_iter()
                .map(|column| {
                    let class = match column.size {
                        ColumnSize::Small => "dashboard-column small",
                        ColumnSize::Full => "dashboard-column",
                    };
                    view! {
                        <div class=class>
                            {column
                                .widgets
                                .into_iter()
                                .map(|instance| view! { <WidgetView instance/> })
                                .collect_view()}
                        </div>
                    }
                })
                .collect_view()}
        </div>
    }
}

#[component]
fn WidgetView(instance: WidgetInstance) -> impl IntoView {
    match instance.widget {
        Widget::Search { search_url } => search_url
            .map(|template| view! { <SearchBar template/> })
            .into_any(),
        Widget::Services => view! { <ServicesWidget/> }.into_any(),
        Widget::Bookmarks { groups } => view! { <Bookmarks groups/> }.into_any(),
        Widget::Calendar => view! { <CalendarWidget/> }.into_any(),
        Widget::Systemd { title, .. } => view! { <SystemdWidget id=instance.id title/> }.into_any(),
        Widget::Weather { location } => view! { <WeatherWidget location/> }.into_any(),
        widget => view! { <DataWidget id=instance.id widget/> }.into_any(),
    }
}

/// The monitored-services grid, re-polled while the dashboard stays open.
#[component]
fn ServicesWidget() -> impl IntoView {
    let client = use_client();

    // Bumped after a start/stop of an on-demand service so its tile flips
    // to the fresh state right away instead of on the next poll.
    let (action, run) = crate::hooks::use_action({
        let client = client.clone();
        move |(id, action): (String, SystemdAction)| {
            let client = client.clone();
            async move { client.service_action(&id, action).await }
        }
    });
    // Owned here, not inside Collapsible, so an expanded list stays expanded
    // when the 30s poll re-renders the widget body.
    let collapsed = RwSignal::new(true);

    let conn = crate::offline::use_connectivity();
    let services = crate::hooks::use_polled_resource(SERVICES_REFRESH, Some(action.version), {
        let client = client.clone();
        move || {
            let client = client.clone();
            async move { crate::offline::cached(conn, "services", client.services()).await }
        }
    });

    // Re-render only when the payload actually changes; a poll returning
    // identical data must not tear down the grid (that recreated every
    // service icon <img> and reset IconOrLetter fallback state). Errors are
    // stringified because chaos_client::ClientError is not PartialEq.
    let services = Memo::new(move |_| services.get().map(|r| r.map_err(|e| e.to_string())));

    view! {
        <section class="widget widget-services">
            <h2>"Services"</h2>
            {move || action.error.get().map(|err| view! { <p class="error">{err}</p> })}
            {move || match services.get() {
                None => view! { <p class="muted">"Checking services…"</p> }.into_any(),
                Some(Ok((mut list, stale))) => {
                    // Cached statuses are from another era; force them honest
                    // (Unknown, no latency) and drop the start/stop controls.
                    if stale {
                        for service in &mut list {
                            service.status.state = HealthState::Unknown;
                            service.status.latency_ms = None;
                        }
                    }
                    let count = list.len();
                    view! {
                        <Collapsible count collapsed>
                            <ServiceGrid services=list controls=(action.busy, run) read_only=stale/>
                        </Collapsible>
                    }
                        .into_any()
                }
                Some(Err(err)) => {
                    view! { <p class="error">"Could not reach chaos server: " {err}</p> }
                        .into_any()
                }
            }}
        </section>
    }
}

/// Weather is fetched directly from Open-Meteo (see weather_fetch) — the
/// only dashboard widget with no server dependency, so it keeps polling
/// even while the server is unreachable.
#[component]
fn WeatherWidget(location: String) -> impl IntoView {
    let data = crate::hooks::use_polled_resource_with(WIDGET_REFRESH, None, true, move || {
        let configured = location.clone();
        async move {
            // Device preference: weather follows the location set on /settings.
            let place = crate::pref(crate::WEATHER_LOCATION_KEY).unwrap_or(configured);
            // The dashboard card doesn't render times, so the place's
            // utc_offset_seconds is dropped here.
            crate::weather_fetch::place_weather(&place)
                .await
                .map(|(data, _)| data)
        }
    });
    let data = Memo::new(move |_| data.get());
    view! {
        <section class="widget widget-weather">
            <h2>
                // The title opens the detailed hourly/multi-location page,
                // like the calendar widget's title opens the calendar.
                <a class="widget-title-link" href="/weather" title="Open weather">"Weather"</a>
            </h2>
            {move || match data.get() {
                None => view! { <p class="muted">"Loading…"</p> }.into_any(),
                Some(Ok(weather)) => view! { <WeatherView weather/> }.into_any(),
                Some(Err(err)) => view! { <p class="error">{err}</p> }.into_any(),
            }}
        </section>
    }
}

/// A widget whose payload comes from `/api/v1/widgets/{id}`.
#[component]
fn DataWidget(id: String, widget: Widget) -> impl IntoView {
    let client = use_client();

    // Kind class so layout variants can reorder/span widgets in CSS.
    let kind = match &widget {
        Widget::Feed { .. } => "feed",
        Widget::HackerNews { .. } => "hacker-news",
        Widget::Lobsters { .. } => "lobsters",
        Widget::Releases { .. } => "releases",
        Widget::ServerStats { .. } => "server-stats",
        _ => "data",
    };
    let title = match &widget {
        Widget::Feed { title, .. } => title.clone().unwrap_or_else(|| "Feed".into()),
        Widget::HackerNews { title, .. } => title.clone().unwrap_or_else(|| "Hacker News".into()),
        Widget::Lobsters { title, .. } => title.clone().unwrap_or_else(|| "Lobsters".into()),
        Widget::Releases { .. } => "Releases".to_string(),
        Widget::ServerStats { .. } => "Server".to_string(),
        _ => String::new(),
    };

    // HN/lobsters can be fetched without the server: HN's Algolia archive
    // API sends CORS `*`, lobsters only works through the shell's HTTP
    // plugin. Cached under the same widget key either way, so each path
    // serves the other's leftovers.
    let direct = match &widget {
        Widget::HackerNews { limit, .. } => Some(DirectFeed::HackerNews(*limit)),
        Widget::Lobsters { limit, .. } => Some(DirectFeed::Lobsters(*limit)),
        _ => None,
    };

    let conn = crate::offline::use_connectivity();
    // `poll_offline: direct.is_some()` — the direct-capable widgets keep
    // their cadence while the server is unreachable; the others pause.
    let data = crate::hooks::use_polled_resource_with(WIDGET_REFRESH, None, direct.is_some(), {
        let client = client.clone();
        move || {
            let client = client.clone();
            let id = id.clone();
            async move {
                let key = format!("widget:{id}");
                if let Some(direct) = direct
                    && conn.get_untracked() != crate::offline::Connectivity::Online
                {
                    return cached_direct(&key, direct.fetch()).await;
                }
                crate::offline::cached(conn, &key, async { client.widget_data(&id).await }).await
            }
        }
    });

    // WidgetData is PartialEq; skip subtree rebuilds when a refresh returns
    // the same cached payload (the server caches these widgets anyway).
    let data = Memo::new(move |_| data.get().map(|r| r.map_err(|e| e.to_string())));
    // Serving from the local cache gets a small "· cached" hint by the title.
    let stale = Memo::new(move |_| matches!(data.get(), Some(Ok((_, true)))));

    // One signal for whichever Collapsible arm renders (Feed or Releases);
    // owned here so poll-driven rebuilds keep the user's expand state. The
    // Posts arm owns its own collapsed/tab state inside PostsBody.
    let collapsed = RwSignal::new(true);

    view! {
        <section class=format!("widget widget-{kind}")>
            <h2>{title} {move || stale.get().then(stale_hint)}</h2>
            {move || match data.get() {
                None => view! { <p class="muted">"Loading…"</p> }.into_any(),
                // Weather renders through WeatherWidget (direct Open-Meteo
                // fetch); the server no longer produces this payload here.
                Some(Ok((WidgetData::Weather(_), _))) => ().into_any(),
                Some(Ok((WidgetData::Feed { items }, _))) => {
                    let count = items.len();
                    let anchor = score_anchor(items.iter().map(|i| i.score));
                    view! {
                        <Collapsible count collapsed>
                            <ul class="feed-list">
                                {items
                                    .into_iter()
                                    .map(|item| feed_item_view(item, anchor))
                                    .collect_view()}
                            </ul>
                        </Collapsible>
                    }
                        .into_any()
                }
                // HN/lobsters window-tops: one pre-computed list per trailing
                // window, switched client-side — tabs cost no refetch.
                Some(Ok((WidgetData::Posts(posts), _))) => {
                    // One anchor over the union of the three windows, so
                    // switching tabs never rescales the colors.
                    let anchor = score_anchor(
                        posts
                            .last_24h
                            .iter()
                            .chain(&posts.last_48h)
                            .chain(&posts.last_week)
                            .map(|i| i.score),
                    );
                    view! { <PostsBody posts anchor/> }.into_any()
                }
                Some(Ok((WidgetData::Releases { items }, _))) => {
                    let count = items.len();
                    view! {
                        <Collapsible count collapsed>
                        <ul class="feed-list">
                            {items
                                .into_iter()
                                .map(|item| {
                                    view! {
                                        <li>
                                            <a
                                                href=item.url.map(|u| u.to_string()).unwrap_or_default()
                                                target="_blank"
                                                rel="noreferrer"
                                            >
                                                <span class="release-repo">{item.repo}</span>
                                                " "
                                                {item.tag}
                                            </a>
                                            <span class="muted">
                                                {item.published.map(rel_time).unwrap_or_default()}
                                            </span>
                                        </li>
                                    }
                                })
                                .collect_view()}
                        </ul>
                        </Collapsible>
                    }
                        .into_any()
                }
                Some(Ok((WidgetData::ServerStats(stats), _))) => {
                    view! { <ServerStatsView stats/> }.into_any()
                }
                // Systemd widgets render through SystemdWidget; this arm only
                // exists for exhaustiveness.
                Some(Ok((WidgetData::Systemd { units }, _))) => {
                    systemd_rows(units, None).into_any()
                }
                Some(Err(err)) => view! { <p class="error">{err}</p> }.into_any(),
            }}
        </section>
    }
}

/// Which trailing window of a Posts widget is shown. Pure display state:
/// all three lists arrive in the payload, so switching never refetches.
#[derive(Clone, Copy, PartialEq)]
enum PostsTab {
    Day,
    TwoDays,
    Week,
}

/// The list shown for a tab. Pure selection over the pre-computed windows,
/// pulled out of the view so it can be unit-tested.
fn posts_window(posts: &PostsData, tab: PostsTab) -> Vec<FeedItem> {
    match tab {
        PostsTab::Day => posts.last_24h.clone(),
        PostsTab::TwoDays => posts.last_48h.clone(),
        PostsTab::Week => posts.last_week.clone(),
    }
}

/// Body of a Posts (HN/lobsters) widget: the 24h/48h/Week tabs and the list
/// they switch between. `tab` and a top-level `Memo` live in this
/// component's owner scope so `tab.set` actually re-renders the list — when
/// this lived in a closure nested inside the type-erased `match` arm the
/// subscription was dropped and clicks did nothing. `collapsed` is owned
/// here too so poll-driven rebuilds keep the user's expand choice.
#[component]
fn PostsBody(posts: PostsData, anchor: Option<u64>) -> impl IntoView {
    let collapsed = RwSignal::new(true);
    let tab = RwSignal::new(PostsTab::Day);
    let shown = Memo::new(move |_| posts_window(&posts, tab.get()));
    view! {
        <div class="posts-tabs">
            {[(PostsTab::Day, "24h"), (PostsTab::TwoDays, "48h"), (PostsTab::Week, "Week")]
                .map(|(t, label)| {
                    view! {
                        <button
                            class:active=move || tab.get() == t
                            on:click=move |_| tab.set(t)
                        >
                            {label}
                        </button>
                    }
                })}
        </div>
        {move || {
            let items = shown.get();
            let count = items.len();
            view! {
                <Collapsible count collapsed>
                    <ul class="feed-list">
                        {items
                            .into_iter()
                            .map(|item| feed_item_view(item, anchor))
                            .collect_view()}
                    </ul>
                </Collapsible>
            }
        }}
    }
}

/// A feed the client can fetch without the chaos server. Only consulted
/// while offline: online, the server's cached copy is authoritative (and
/// the lobsters path only exists inside the shells anyway).
#[derive(Clone, Copy)]
enum DirectFeed {
    HackerNews(u32),
    Lobsters(u32),
}

impl DirectFeed {
    async fn fetch(self) -> Result<WidgetData, ClientError> {
        let now = chrono::Utc::now();
        let posts = match self {
            DirectFeed::HackerNews(limit) => {
                chaos_client::posts::hacker_news(&crate::weather_fetch::http(), limit, now).await
            }
            DirectFeed::Lobsters(limit) => lobsters_direct(limit, now).await,
        }
        .map_err(ClientError::Transport)?;
        Ok(WidgetData::Posts(posts))
    }
}

/// Page through newest.json via the shell's HTTP plugin (lobste.rs sends no
/// CORS headers) until the sweep covers a week or hits the page cap.
async fn lobsters_direct(
    limit: u32,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<chaos_domain::PostsData, String> {
    use chaos_client::posts;
    let mut items = Vec::new();
    for page in 1..=posts::LOBSTERS_PAGE_CAP {
        let json = match crate::tauri_http::fetch_text(&posts::lobsters_page_url(page)).await {
            Some(Ok(json)) => json,
            Some(Err(err)) => return Err(err),
            None => return Err("lobsters needs the app shell offline".into()),
        };
        let page_items = posts::parse_lobsters_page(&json)?;
        if page_items.is_empty() {
            break;
        }
        let done = posts::lobsters_page_done(&page_items, now);
        items.extend(page_items);
        if done {
            break;
        }
    }
    if items.is_empty() {
        return Err("no stories returned".into());
    }
    Ok(posts::posts_from_items(&items, limit, now))
}

/// [`crate::offline::cached`] for a fetch that does NOT go through the
/// chaos server. `cached()` serves the cache immediately whenever the app
/// is not Online — exactly wrong here, since offline is when the direct
/// fetch must actually run. So: fetch first, overwrite the cache on
/// success (same widget key, so the server path later serves these
/// leftovers and vice versa), fall back to the cached copy on failure.
/// Connectivity is left untouched — a direct-fetch failure says nothing
/// about the chaos server. Same `(value, stale)` shape as `cached()`: a
/// fresh direct fetch is NOT stale, so it gets no "· cached" hint.
async fn cached_direct(
    key: &str,
    fetch: impl Future<Output = Result<WidgetData, ClientError>>,
) -> Result<(WidgetData, bool), ClientError> {
    match fetch.await {
        Ok(value) => {
            crate::offline::cache_put(key, &value);
            Ok((value, false))
        }
        Err(err) => match crate::offline::cache_get::<WidgetData>(key) {
            Some(hit) => Ok((hit, true)),
            None => Err(err),
        },
    }
}

/// Wrapper collapsing a long list to its first three entries on phones —
/// the hiding and the ▾/▴ toggle are CSS, scoped to the narrow layout, so
/// wider screens always show everything.
/// The `collapsed` signal is owned by the widget (not this component) so
/// the user's expand/collapse choice survives the poll-driven rebuilds of
/// the widget body.
#[component]
fn Collapsible(count: usize, collapsed: RwSignal<bool>, children: Children) -> impl IntoView {
    view! {
        <div class="collapsible" class:collapsed=move || collapsed.get()>
            {children()}
            {(count > 3)
                .then(|| {
                    view! {
                        <button
                            class="collapse-toggle"
                            title="Show more or less"
                            on:click=move |_| collapsed.update(|c| *c = !*c)
                        >
                            {move || if collapsed.get() { "▾" } else { "▴" }}
                        </button>
                    }
                })}
        </div>
    }
}

/// Systemd unit states with optional start/stop/restart controls.
#[component]
fn SystemdWidget(id: String, title: Option<String>) -> impl IntoView {
    let client = use_client();
    let title = title.unwrap_or_else(|| "Service control".into());

    // Unit states change on their own (crashes, timers), so poll like the
    // services grid does; a successful control action bumps the version.
    let (action, run) = crate::hooks::use_action({
        let client = client.clone();
        let id = id.clone();
        move |(unit, action): (String, SystemdAction)| {
            let client = client.clone();
            let id = id.clone();
            async move {
                client
                    .systemd_action(&id, &SystemdActionRequest { unit, action })
                    .await
            }
        }
    });

    let conn = crate::offline::use_connectivity();
    let data = crate::hooks::use_polled_resource(SERVICES_REFRESH, Some(action.version), {
        let client = client.clone();
        let id = id.clone();
        move || {
            let client = client.clone();
            let id = id.clone();
            async move {
                crate::offline::cached(conn, &format!("widget:{id}"), async {
                    client.widget_data(&id).await
                })
                .await
            }
        }
    });

    // Unit states rarely change between polls; only rebuild the rows (and
    // their control buttons) when they do.
    let data = Memo::new(move |_| data.get().map(|r| r.map_err(|e| e.to_string())));
    let stale = Memo::new(move |_| matches!(data.get(), Some(Ok((_, true)))));

    view! {
        <section class="widget widget-systemd">
            <h2>{title} {move || stale.get().then(stale_hint)}</h2>
            {move || action.error.get().map(|err| view! { <p class="error">{err}</p> })}
            {move || match data.get() {
                None => view! { <p class="muted">"Loading…"</p> }.into_any(),
                Some(Ok((WidgetData::Systemd { units }, stale))) => {
                    // Cached unit states can't be acted on; drop the buttons.
                    let controls = (!stale).then_some((action.busy, run));
                    systemd_rows(units, controls).into_any()
                }
                Some(Ok(_)) => view! { <p class="error">"Unexpected widget data"</p> }.into_any(),
                Some(Err(err)) => view! { <p class="error">{err}</p> }.into_any(),
            }}
        </section>
    }
}

/// Small marker next to a widget title while it renders cached (offline)
/// data — the payload may be arbitrarily old.
fn stale_hint() -> impl IntoView {
    view! { <span class="muted stale-hint">" · cached"</span> }
}

fn systemd_rows(units: Vec<SystemdUnitStatus>, controls: Option<SystemdControls>) -> impl IntoView {
    view! {
        <ul class="unit-list">
            {units
                .into_iter()
                .map(|unit| {
                    let dot = match unit.active_state.as_str() {
                        "active" => "dot up",
                        "failed" => "dot down",
                        "activating" | "deactivating" | "reloading" => "dot degraded",
                        _ => "dot unknown",
                    };
                    let state = if unit.sub_state.is_empty() {
                        unit.active_state.clone()
                    } else {
                        unit.sub_state.clone()
                    };
                    let actions = controls
                        .filter(|_| unit.controllable)
                        .map(|(busy, run)| {
                            unit_actions(
                                unit.unit.clone(),
                                unit.active_state == "active",
                                busy,
                                run,
                            )
                        });
                    view! {
                        <li class="unit-row">
                            <span class=dot title=unit.active_state.clone()></span>
                            <span class="unit-title">{unit.title}</span>
                            <span class="muted unit-state">{state}</span>
                            {actions}
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    }
}

fn unit_actions(
    unit: String,
    active: bool,
    busy: RwSignal<bool>,
    run: Callback<(String, SystemdAction)>,
) -> impl IntoView {
    let button = move |label: &'static str, title: &'static str, action: SystemdAction| {
        let unit = unit.clone();
        view! {
            <button
                class="unit-btn"
                title=title
                disabled=move || busy.get()
                on:click=move |_| run.run((unit.clone(), action))
            >
                {label}
            </button>
        }
    };
    view! {
        <span class="unit-actions">
            {if active {
                vec![
                    button("↻", "Restart", SystemdAction::Restart),
                    button("■", "Stop", SystemdAction::Stop),
                ]
            } else {
                vec![button("▶", "Start", SystemdAction::Start)]
            }}
        </span>
    }
}

/// Static month calendar, computed entirely client-side.
#[component]
fn CalendarWidget() -> impl IntoView {
    let today = Local::now().date_naive();
    let month = RwSignal::new((today.year(), today.month()));

    let shift = move |delta: i32| {
        month.update(|(year, m)| {
            (*year, *m) = crate::date_util::shift_month(*year, *m, delta);
        });
    };

    view! {
        <section class="widget widget-calendar">
            <div class="calendar-head">
                <h2>
                    // Clicking the title opens the full calendar section.
                    <a class="widget-title-link" href="/calendar" title="Open calendar">
                        {move || {
                            let (year, m) = month.get();
                            NaiveDate::from_ymd_opt(year, m, 1)
                                .map(|d| d.format("%B %Y").to_string())
                                .unwrap_or_default()
                        }}
                    </a>
                </h2>
                <div class="calendar-nav">
                    <button title="Previous month" on:click=move |_| shift(-1)>"‹"</button>
                    <button
                        title="Current month"
                        on:click=move |_| month.set((today.year(), today.month()))
                    >
                        "•"
                    </button>
                    <button title="Next month" on:click=move |_| shift(1)>"›"</button>
                </div>
            </div>
            <div class="calendar-grid">
                {["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"]
                    .into_iter()
                    .map(|day| view! { <span class="calendar-weekday muted">{day}</span> })
                    .collect_view()}
                {move || calendar_cells(month.get(), today)}
            </div>
        </section>
    }
}

/// Six fixed weeks around the shown month, starting on Monday.
fn calendar_cells((year, month): (i32, u32), today: NaiveDate) -> impl IntoView {
    let Some(days) = crate::date_util::month_grid(year, month) else {
        return ().into_any();
    };
    days.map(|date| {
        let mut class = String::from("calendar-cell");
        if date.month() != month {
            class.push_str(" other");
        }
        if date == today {
            class.push_str(" today");
        }
        view! { <span class=class>{date.day()}</span> }
    })
    .collect_view()
    .into_any()
}

#[component]
fn WeatherView(weather: WeatherData) -> impl IntoView {
    // °F/°C is a device display preference (/settings), defaulting to what
    // the system locale suggests; the wire stays metric.
    let fahrenheit = crate::weather_fahrenheit();
    let temp = move |celsius: f64| crate::format_temp(celsius, fahrenheit);
    let details = crate::weather_details(&weather.location, &weather, fahrenheit);
    view! {
        <div class="weather">
            <div class="weather-now">
                <span class="weather-emoji">{crate::weather_emoji(weather.weather_code)}</span>
                <span class="weather-temp">{temp(weather.temperature_c)}</span>
                <div class="weather-desc">
                    <div>{weather.description}</div>
                    <div class="muted">{details}</div>
                </div>
            </div>
            <div class="weather-days">
                {weather
                    .daily
                    .into_iter()
                    .map(|day| {
                        view! {
                            <div class="weather-day">
                                <span class="muted">{day.date.format("%a").to_string()}</span>
                                <span>{crate::weather_emoji(day.weather_code)}</span>
                                <span>{temp(day.max_c)}</span>
                                <span class="muted">{temp(day.min_c)}</span>
                            </div>
                        }
                    })
                    .collect_view()}
            </div>
        </div>
    }
}

#[component]
fn ServerStatsView(stats: ServerStats) -> impl IntoView {
    let up = stats.uptime_secs;
    let uptime = if up >= 86_400 {
        format!("{}d {}h", up / 86_400, (up % 86_400) / 3_600)
    } else {
        format!("{}h {}m", up / 3_600, (up % 3_600) / 60)
    };
    let head = format!(
        "{} · up {} · load {:.2} {:.2} {:.2}",
        stats.hostname.unwrap_or_else(|| "host".into()),
        uptime,
        stats.load_avg[0],
        stats.load_avg[1],
        stats.load_avg[2],
    );

    let sparks = (!stats.history.is_empty()).then(|| {
        let cpu: Vec<f32> = stats.history.iter().map(|p| p.cpu_pct / 100.0).collect();
        let mem: Vec<f32> = stats
            .history
            .iter()
            .map(|p| p.mem_used_bytes as f32 / stats.mem_total_bytes.max(1) as f32)
            .collect();
        let cpu_now = format!("{:.0}%", stats.history.last().map_or(0.0, |p| p.cpu_pct));
        let minutes = stats.history.len() as u64 * ServerStats::HISTORY_INTERVAL_SECS / 60;
        view! {
            <div class="spark-row">
                <div class="spark">
                    <span class="spark-head muted">"cpu " {cpu_now} " · " {minutes} "m"</span>
                    <Sparkline values=cpu/>
                </div>
                <div class="spark">
                    <span class="spark-head muted">"memory"</span>
                    <Sparkline values=mem/>
                </div>
            </div>
        }
    });

    view! {
        <div class="server-stats">
            <p class="muted">{head}</p>
            {sparks}
            // One shared grid so labels/values form real columns and every
            // bar has the same width regardless of the text around it.
            <div class="meters">
                <Meter
                    label="memory".to_string()
                    used=stats.mem_used_bytes
                    total=stats.mem_total_bytes
                />
                {stats
                    .disks
                    .into_iter()
                    .map(|disk| {
                        view! {
                            <Meter label=disk.mount used=disk.used_bytes total=disk.total_bytes/>
                        }
                    })
                    .collect_view()}
            </div>
        </div>
    }
}

/// Heat-gradient stops for score coloring, coldest (t = 0) to hottest
/// (t = 1): faint yellow → yellow → orange → soft red → hard red.
const HEAT_STOPS: [(u8, u8, u8); 5] = [
    (0xe8, 0xd2, 0x88),
    (0xff, 0xd6, 0x0a),
    (0xff, 0x95, 0x00),
    (0xf4, 0x67, 0x4f),
    (0xff, 0x22, 0x00),
];

/// Top of the score color scale: the nearest-rank 99th percentile of the
/// scores present (sort ascending, take index `ceil(0.99·n) - 1`). `None`
/// when nothing in scope carries a score. For n ≤ 30 this is simply the
/// max; on larger populations it lets a lone viral outlier stay clamped
/// at hard red without washing out everything below it.
fn score_anchor(scores: impl IntoIterator<Item = Option<u64>>) -> Option<u64> {
    let mut scores: Vec<u64> = scores.into_iter().flatten().collect();
    if scores.is_empty() {
        return None;
    }
    scores.sort_unstable();
    let rank = (0.99 * scores.len() as f64).ceil() as usize; // ≥ 1 since n ≥ 1
    Some(scores[rank - 1])
}

/// `#rrggbb` heat color for a score: `t = ln(1+score) / ln(1+anchor)`
/// clamped to [0, 1], linearly interpolated in RGB between the adjacent
/// [`HEAT_STOPS`]. The logarithmic normalization spreads the clustered
/// mid-range scores apart instead of clumping them at the faint end.
/// Anchor 0 (guarding the division) renders the faint end.
fn score_color(score: u64, anchor: u64) -> String {
    let t = if anchor == 0 {
        0.0
    } else {
        (((score as f64) + 1.0).ln() / ((anchor as f64) + 1.0).ln()).clamp(0.0, 1.0)
    };
    let pos = t * (HEAT_STOPS.len() - 1) as f64;
    let i = (pos as usize).min(HEAT_STOPS.len() - 2);
    let f = pos - i as f64;
    let (lo, hi) = (HEAT_STOPS[i], HEAT_STOPS[i + 1]);
    let lerp = |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * f).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        lerp(lo.0, hi.0),
        lerp(lo.1, hi.1),
        lerp(lo.2, hi.2)
    )
}

/// One feed/aggregator entry: title → article, source label → discussion
/// page (HN/lobsters), plus points, comment count and age when available.
/// Each meta part is its own span so the aggregator widgets can align them
/// as columns across rows (see `.feed-meta` in the stylesheet). `anchor`
/// (the caller's [`score_anchor`] over the whole widget) heat-colors the
/// points; without one — or on scoreless items — the muted styling stands.
fn feed_item_view(item: FeedItem, anchor: Option<u64>) -> impl IntoView {
    let source = item
        .source
        .filter(|s| !s.is_empty())
        .map(|s| match &item.comments_url {
            Some(url) => view! {
                <a class="feed-source" href=url.to_string() target="_blank" rel="noreferrer">
                    {s}
                </a>
            }
            .into_any(),
            None => view! { <span class="feed-source">{s}</span> }.into_any(),
        });
    let score = item.score.map(|score| format!("▲ {score}"));
    let score_style = match (item.score, anchor) {
        (Some(score), Some(anchor)) => Some(score_color(score, anchor)),
        _ => None,
    };
    let comments = item
        .comments
        .map(|n| format!("{n} comment{}", if n == 1 { "" } else { "s" }));
    let age = item.published.map(rel_time);

    view! {
        <li>
            <a
                href=item.url.map(|u| u.to_string()).unwrap_or_default()
                target="_blank"
                rel="noreferrer"
            >
                {item.title}
            </a>
            <span class="muted feed-meta">
                {source}
                <span class="feed-score" style:color=score_style>
                    {score}
                </span>
                <span class="feed-comments">{comments}</span>
                <span class="feed-age">{age}</span>
            </span>
        </li>
    }
}

/// Tiny history graph (values normalized to 0..=1, oldest first). Drawn as
/// an area + line in a stretched SVG so it costs nothing to render.
#[component]
fn Sparkline(values: Vec<f32>) -> impl IntoView {
    let n = values.len();
    let points: String = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = if n > 1 {
                i as f32 / (n - 1) as f32
            } else {
                0.5
            } * 100.0;
            let y = 100.0 - v.clamp(0.0, 1.0) * 100.0;
            format!("{x:.1},{y:.1} ")
        })
        .collect();
    let area = format!("0,100 {points}100,100");
    view! {
        <svg class="sparkline" viewBox="0 0 100 100" preserveAspectRatio="none">
            <polygon class="sparkline-area" points=area></polygon>
            <polyline class="sparkline-line" points=points></polyline>
        </svg>
    }
}

/// Labelled usage bar (memory, disks).
#[component]
fn Meter(label: String, used: u64, total: u64) -> impl IntoView {
    let pct = if total == 0 {
        0.0
    } else {
        used as f64 / total as f64 * 100.0
    };
    let class = if pct >= 90.0 {
        "meter-fill high"
    } else {
        "meter-fill"
    };
    view! {
        <div class="meter-row">
            <span class="meter-label">{label}</span>
            <div class="meter">
                <div class=class style=format!("width: {pct:.0}%")></div>
            </div>
            <span class="meter-value muted">
                {format!("{} / {}", format_bytes(used), format_bytes(total))}
            </span>
        </div>
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn rel_time(when: DateTime<Utc>) -> String {
    let minutes = (Utc::now() - when).num_minutes().max(0);
    match minutes {
        0..60 => format!("{minutes}m"),
        60..1440 => format!("{}h", minutes / 60),
        _ => format!("{}d", minutes / 1440),
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
        // shells: system browser; plain web: a normal new tab
        if !crate::open_external(&target) {
            let _ = window().open_with_url_and_target(&target, "_blank");
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    // -- score_anchor: nearest-rank 99th percentile ------------------------

    #[test]
    fn anchor_single_score() {
        assert_eq!(score_anchor([Some(42)]), Some(42));
    }

    #[test]
    fn anchor_n10_is_max() {
        // ceil(0.99 * 10) = 10 → last element after ascending sort.
        let scores = (1..=10).map(Some);
        assert_eq!(score_anchor(scores), Some(10));
    }

    #[test]
    fn anchor_all_equal() {
        assert_eq!(score_anchor(vec![Some(7); 25]), Some(7));
    }

    #[test]
    fn anchor_small_n_includes_outlier() {
        // With n ≤ 30, ceil(0.99·n) = n, so p99 is honestly just the max —
        // an outlier still becomes the anchor at these sizes.
        let mut scores = vec![Some(5); 29];
        scores.push(Some(10_000));
        assert_eq!(score_anchor(scores), Some(10_000));
    }

    #[test]
    fn anchor_large_n_shrugs_off_outlier() {
        // n = 200: ceil(0.99 · 200) = 198 → index 197, below the outlier.
        let mut scores = vec![Some(10); 199];
        scores.push(Some(10_000));
        assert_eq!(score_anchor(scores), Some(10));
    }

    #[test]
    fn anchor_ignores_scoreless_items() {
        assert_eq!(score_anchor([None, Some(3), None, Some(9)]), Some(9));
    }

    #[test]
    fn anchor_none_without_scores() {
        assert_eq!(score_anchor([None, None]), None);
        assert_eq!(score_anchor([]), None);
    }

    // -- score_color: five-stop heat gradient ------------------------------

    #[test]
    fn color_hits_the_five_stops_exactly() {
        // Under the log scale, anchor 15 (ln 16 = 4·ln 2) lands scores
        // 0/1/3/7/15 on t = 0/¼/½/¾/1 — exactly the five stops.
        assert_eq!(score_color(0, 15), "#e8d288");
        assert_eq!(score_color(1, 15), "#ffd60a");
        assert_eq!(score_color(3, 15), "#ff9500");
        assert_eq!(score_color(7, 15), "#f4674f");
        assert_eq!(score_color(15, 15), "#ff2200");
    }

    #[test]
    fn color_midpoint_between_stops() {
        // t = ln 2 / ln 256 = 0.125, halfway from #e8d288 to #ffd60a:
        // r (232+255)/2 → 244, g (210+214)/2 → 212, b (136+10)/2 → 73.
        assert_eq!(score_color(1, 255), "#f4d449");
    }

    #[test]
    fn color_clamps_above_anchor() {
        assert_eq!(score_color(9999, 100), "#ff2200");
    }

    #[test]
    fn color_anchor_zero_renders_faint_end() {
        assert_eq!(score_color(0, 0), "#e8d288");
        assert_eq!(score_color(50, 0), "#e8d288");
    }

    // -- score_color: logarithmic scale -----------------------------------

    #[test]
    fn log_scale_separates_midrange_scores() {
        // Real clustered set: anchor is the top score.
        let anchor = 2146u64;
        // Under linear score/anchor, 334 and 497 both sit ~0.15-0.23 → both in the
        // first gradient segment (faint→yellow). Under log they diverge.
        let c334 = score_color(334, anchor);
        let c497 = score_color(497, anchor);
        assert_ne!(c334, c497, "mid-range scores must be distinguishable");
    }

    #[test]
    fn log_scale_is_monotonic() {
        let anchor = 2146u64;
        // Higher score → not-lighter color: compare the red channel (first two hex
        // chars), which increases toward hard red.
        let red = |s: &str| u8::from_str_radix(&s[1..3], 16).unwrap();
        assert!(red(&score_color(865, anchor)) >= red(&score_color(193, anchor)));
        assert!(red(&score_color(2146, anchor)) >= red(&score_color(865, anchor)));
    }

    #[test]
    fn log_scale_anchor_hits_hard_red() {
        assert_eq!(score_color(2146, 2146), "#ff2200");
    }

    #[test]
    fn log_scale_overflow_clamps_to_hard_red() {
        assert_eq!(score_color(9000, 2146), "#ff2200");
    }

    #[test]
    fn log_scale_zero_anchor_is_faint() {
        assert_eq!(score_color(0, 0), "#e8d288");
    }

    #[test]
    fn log_scale_lifts_low_score_out_of_yellow() {
        // Real clustered set: 181 against anchor 2146. Linear 181/2146 = 0.084
        // → pos 0.34, stuck in the faint→yellow first segment (green ≈ 211).
        // Log ln(182)/ln(2147) = 0.68 → pos 2.71, the orange→red segment
        // (green ≈ 116). Asserting it left the yellow band FAILS under linear.
        let green = |s: &str| u8::from_str_radix(&s[3..5], 16).unwrap();
        assert!(
            green(&score_color(181, 2146)) < 150,
            "low score must reach the orange/red band under the log scale"
        );
    }

    // -- posts_window: tab selects the matching trailing window ------------

    #[test]
    fn posts_window_selects_by_tab() {
        let mk = |title: &str| FeedItem {
            title: title.into(),
            url: None,
            source: None,
            published: None,
            score: None,
            comments: None,
            comments_url: None,
        };
        let posts = PostsData {
            last_24h: vec![mk("a")],
            last_48h: vec![mk("a"), mk("b")],
            last_week: vec![mk("a"), mk("b"), mk("c")],
        };
        assert_eq!(posts_window(&posts, PostsTab::Day).len(), 1);
        assert_eq!(posts_window(&posts, PostsTab::TwoDays).len(), 2);
        assert_eq!(posts_window(&posts, PostsTab::Week).len(), 3);
    }
}

#[component]
fn Bookmarks(groups: Vec<BookmarkGroup>) -> impl IntoView {
    let client = use_client();

    view! {
        <section class="widget widget-bookmarks">
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
                                            let title = bookmark.title.clone();
                                            let url = bookmark.url.to_string();
                                            let package = bookmark.android_package.clone();
                                            let on_click = move |ev: leptos::ev::MouseEvent| {
                                                if let Some(package) = &package
                                                    && crate::on_android()
                                                    && crate::open_app_native(package)
                                                {
                                                    // The native app claimed the tap.
                                                    ev.prevent_default();
                                                }
                                                // Otherwise the anchor's target="_blank"
                                                // does the right thing everywhere.
                                            };
                                            view! {
                                                <li>
                                                    <a
                                                        href=url
                                                        target="_blank"
                                                        rel="noreferrer"
                                                        on:click=on_click
                                                    >
                                                        <crate::components::IconOrLetter
                                                            url=icon.map(|u| u.to_string())
                                                            title=title
                                                            class="bookmark-icon"
                                                        />
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
