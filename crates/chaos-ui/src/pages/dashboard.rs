use std::time::Duration;

use chaos_domain::{
    BookmarkGroup, ColumnSize, DashboardLayout, FeedItem, ServerStats, SystemdAction,
    SystemdActionRequest, SystemdUnitStatus, WeatherData, Widget, WidgetData, WidgetInstance,
};
use chrono::{DateTime, Datelike, Days, Local, NaiveDate, Utc};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::components::ServiceGrid;
use crate::use_client;

const SERVICES_REFRESH: Duration = Duration::from_secs(30);
/// Data widgets are cached server-side; this only controls how often an open
/// dashboard picks the fresh cache up.
const WIDGET_REFRESH: Duration = Duration::from_secs(300);

/// Bumped by the manual refresh button; every widget resource tracks it.
#[derive(Clone, Copy)]
struct RefreshTick(RwSignal<u32>);

/// Busy flag + action dispatcher backing the systemd control buttons.
type SystemdControls = (RwSignal<bool>, Callback<(String, SystemdAction)>);

#[component]
pub fn Dashboard() -> impl IntoView {
    let client = use_client();
    let refresh = RwSignal::new(0u32);
    provide_context(RefreshTick(refresh));

    let layout = LocalResource::new({
        let client = client.clone();
        move || {
            let client = client.clone();
            async move { client.dashboard().await }
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
        widget => view! { <DataWidget id=instance.id widget/> }.into_any(),
    }
}

/// The monitored-services grid, re-polled while the dashboard stays open.
#[component]
fn ServicesWidget() -> impl IntoView {
    let client = use_client();
    let refresh = use_context::<RefreshTick>();

    let tick = RwSignal::new(0u32);
    if let Ok(handle) = set_interval_with_handle(move || tick.update(|n| *n += 1), SERVICES_REFRESH)
    {
        on_cleanup(move || handle.clear());
    }

    // Bumped after a start/stop of an on-demand service so its tile flips
    // to the fresh state right away instead of on the next poll.
    let version = RwSignal::new(0u32);
    let busy = RwSignal::new(false);
    let action_error = RwSignal::new(None::<String>);

    let services = LocalResource::new({
        let client = client.clone();
        move || {
            tick.track();
            version.track();
            if let Some(RefreshTick(refresh)) = refresh {
                refresh.track();
            }
            let client = client.clone();
            async move { client.services().await }
        }
    });

    let run = Callback::new(move |(id, action): (String, SystemdAction)| {
        let client = client.clone();
        busy.set(true);
        action_error.set(None);
        spawn_local(async move {
            match client.service_action(&id, action).await {
                Ok(_) => version.update(|n| *n += 1),
                Err(err) => action_error.set(Some(err.to_string())),
            }
            busy.set(false);
        });
    });

    view! {
        <section class="widget widget-services">
            <h2>"Services"</h2>
            {move || action_error.get().map(|err| view! { <p class="error">{err}</p> })}
            {move || match services.get() {
                None => view! { <p class="muted">"Checking services…"</p> }.into_any(),
                Some(Ok(list)) => {
                    let count = list.len();
                    view! {
                        <Collapsible count>
                            <ServiceGrid services=list controls=(busy, run)/>
                        </Collapsible>
                    }
                        .into_any()
                }
                Some(Err(err)) => {
                    view! { <p class="error">"Could not reach chaos server: " {err.to_string()}</p> }
                        .into_any()
                }
            }}
        </section>
    }
}

/// A widget whose payload comes from `/api/v1/widgets/{id}`.
#[component]
fn DataWidget(id: String, widget: Widget) -> impl IntoView {
    let client = use_client();
    let refresh = use_context::<RefreshTick>();

    let tick = RwSignal::new(0u32);
    if let Ok(handle) = set_interval_with_handle(move || tick.update(|n| *n += 1), WIDGET_REFRESH) {
        on_cleanup(move || handle.clear());
    }

    // Kind class so layout variants can reorder/span widgets in CSS.
    let kind = match &widget {
        Widget::Weather { .. } => "weather",
        Widget::Feed { .. } => "feed",
        Widget::HackerNews { .. } => "hacker-news",
        Widget::Lobsters { .. } => "lobsters",
        Widget::Releases { .. } => "releases",
        Widget::ServerStats { .. } => "server-stats",
        _ => "data",
    };
    let title = match &widget {
        Widget::Weather { .. } => "Weather".to_string(),
        Widget::Feed { title, .. } => title.clone().unwrap_or_else(|| "Feed".into()),
        Widget::HackerNews { title, .. } => title.clone().unwrap_or_else(|| "Hacker News".into()),
        Widget::Lobsters { title, .. } => title.clone().unwrap_or_else(|| "Lobsters".into()),
        Widget::Releases { .. } => "Releases".to_string(),
        Widget::ServerStats { .. } => "Server".to_string(),
        _ => String::new(),
    };

    // The weather title opens the detailed hourly/multi-location page,
    // like the calendar widget's title opens the calendar.
    let title = if matches!(widget, Widget::Weather { .. }) {
        view! {
            <a class="widget-title-link" href="/weather" title="Open weather">
                {title}
            </a>
        }
        .into_any()
    } else {
        title.into_any()
    };

    // Device preference: weather follows the location set on /settings.
    let weather_location =
        matches!(widget, Widget::Weather { .. }).then(|| crate::pref(crate::WEATHER_LOCATION_KEY));
    let data = LocalResource::new(move || {
        tick.track();
        if let Some(RefreshTick(refresh)) = refresh {
            refresh.track();
        }
        let client = client.clone();
        let id = id.clone();
        let location = weather_location.clone().flatten();
        async move { client.widget_data(&id, location.as_deref()).await }
    });

    view! {
        <section class=format!("widget widget-{kind}")>
            <h2>{title}</h2>
            {move || match data.get() {
                None => view! { <p class="muted">"Loading…"</p> }.into_any(),
                Some(Ok(WidgetData::Weather(weather))) => {
                    view! { <WeatherView weather/> }.into_any()
                }
                Some(Ok(WidgetData::Feed { items })) => {
                    let count = items.len();
                    view! {
                        <Collapsible count>
                            <ul class="feed-list">
                                {items.into_iter().map(feed_item_view).collect_view()}
                            </ul>
                        </Collapsible>
                    }
                        .into_any()
                }
                Some(Ok(WidgetData::Releases { items })) => {
                    let count = items.len();
                    view! {
                        <Collapsible count>
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
                Some(Ok(WidgetData::ServerStats(stats))) => {
                    view! { <ServerStatsView stats/> }.into_any()
                }
                // Systemd widgets render through SystemdWidget; this arm only
                // exists for exhaustiveness.
                Some(Ok(WidgetData::Systemd { units })) => systemd_rows(units, None).into_any(),
                Some(Err(err)) => view! { <p class="error">{err.to_string()}</p> }.into_any(),
            }}
        </section>
    }
}

/// Wrapper collapsing a long list to its first three entries on phones —
/// the hiding and the ▾/▴ toggle are CSS, scoped to the narrow layout, so
/// wider screens always show everything.
#[component]
fn Collapsible(count: usize, children: Children) -> impl IntoView {
    let collapsed = RwSignal::new(true);
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
    let refresh = use_context::<RefreshTick>();
    let title = title.unwrap_or_else(|| "Service control".into());

    // Unit states change on their own (crashes, timers), so poll like the
    // services grid does; `version` bumps after our own control actions.
    let tick = RwSignal::new(0u32);
    if let Ok(handle) = set_interval_with_handle(move || tick.update(|n| *n += 1), SERVICES_REFRESH)
    {
        on_cleanup(move || handle.clear());
    }
    let version = RwSignal::new(0u32);
    let busy = RwSignal::new(false);
    let action_error = RwSignal::new(None::<String>);

    let data = LocalResource::new({
        let client = client.clone();
        let id = id.clone();
        move || {
            tick.track();
            version.track();
            if let Some(RefreshTick(refresh)) = refresh {
                refresh.track();
            }
            let client = client.clone();
            let id = id.clone();
            async move { client.widget_data(&id, None).await }
        }
    });

    let run = Callback::new(move |(unit, action): (String, SystemdAction)| {
        let client = client.clone();
        let id = id.clone();
        busy.set(true);
        action_error.set(None);
        spawn_local(async move {
            match client
                .systemd_action(&id, &SystemdActionRequest { unit, action })
                .await
            {
                Ok(_) => version.update(|n| *n += 1),
                Err(err) => action_error.set(Some(err.to_string())),
            }
            busy.set(false);
        });
    });

    view! {
        <section class="widget widget-systemd">
            <h2>{title}</h2>
            {move || action_error.get().map(|err| view! { <p class="error">{err}</p> })}
            {move || match data.get() {
                None => view! { <p class="muted">"Loading…"</p> }.into_any(),
                Some(Ok(WidgetData::Systemd { units })) => {
                    systemd_rows(units, Some((busy, run))).into_any()
                }
                Some(Ok(_)) => view! { <p class="error">"Unexpected widget data"</p> }.into_any(),
                Some(Err(err)) => view! { <p class="error">{err.to_string()}</p> }.into_any(),
            }}
        </section>
    }
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
            let total = *year * 12 + (*m as i32 - 1) + delta;
            *year = total.div_euclid(12);
            *m = (total.rem_euclid(12) + 1) as u32;
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
    let Some(first) = NaiveDate::from_ymd_opt(year, month, 1) else {
        return ().into_any();
    };
    let offset = first.weekday().num_days_from_monday() as u64;
    let start = first - Days::new(offset);

    (0..42u64)
        .map(|i| {
            let date = start + Days::new(i);
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
    let details = format!(
        "{} · feels {} · wind {:.0} km/h{}",
        weather.location,
        temp(weather.apparent_c),
        weather.wind_kmh,
        weather
            .humidity_pct
            .map(|h| format!(" · {h:.0}% humidity"))
            .unwrap_or_default(),
    );
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

/// One feed/aggregator entry: title → article, source label → discussion
/// page (HN/lobsters), plus points, comment count and age when available.
/// Each meta part is its own span so the aggregator widgets can align them
/// as columns across rows (see `.feed-meta` in the stylesheet).
fn feed_item_view(item: FeedItem) -> impl IntoView {
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
                <span class="feed-score">{score}</span>
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
