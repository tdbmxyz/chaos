use std::time::Duration;

use chaos_domain::{
    BookmarkGroup, ColumnSize, DashboardLayout, ServerStats, WeatherData, Widget, WidgetData,
    WidgetInstance,
};
use chrono::{DateTime, Utc};
use leptos::prelude::*;

use crate::components::ServiceGrid;
use crate::use_client;

const SERVICES_REFRESH: Duration = Duration::from_secs(30);
/// Data widgets are cached server-side; this only controls how often an open
/// dashboard picks the fresh cache up.
const WIDGET_REFRESH: Duration = Duration::from_secs(300);

/// Bumped by the manual refresh button; every widget resource tracks it.
#[derive(Clone, Copy)]
struct RefreshTick(RwSignal<u32>);

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

    let services = LocalResource::new({
        let client = client.clone();
        move || {
            tick.track();
            if let Some(RefreshTick(refresh)) = refresh {
                refresh.track();
            }
            let client = client.clone();
            async move { client.services().await }
        }
    });

    view! {
        <section class="widget">
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

    let title = match &widget {
        Widget::Weather { .. } => "Weather".to_string(),
        Widget::Feed { title, .. } => title.clone().unwrap_or_else(|| "Feed".into()),
        Widget::Releases { .. } => "Releases".to_string(),
        Widget::ServerStats { .. } => "Server".to_string(),
        _ => String::new(),
    };

    let data = LocalResource::new(move || {
        tick.track();
        if let Some(RefreshTick(refresh)) = refresh {
            refresh.track();
        }
        let client = client.clone();
        let id = id.clone();
        async move { client.widget_data(&id).await }
    });

    view! {
        <section class="widget">
            <h2>{title}</h2>
            {move || match data.get() {
                None => view! { <p class="muted">"Loading…"</p> }.into_any(),
                Some(Ok(WidgetData::Weather(weather))) => {
                    view! { <WeatherView weather/> }.into_any()
                }
                Some(Ok(WidgetData::Feed { items })) => {
                    view! {
                        <ul class="feed-list">
                            {items
                                .into_iter()
                                .map(|item| {
                                    let meta = [
                                        item.source.clone().unwrap_or_default(),
                                        item.published.map(rel_time).unwrap_or_default(),
                                    ]
                                    .into_iter()
                                    .filter(|part| !part.is_empty())
                                    .collect::<Vec<_>>()
                                    .join(" · ");
                                    view! {
                                        <li>
                                            <a
                                                href=item.url.map(|u| u.to_string()).unwrap_or_default()
                                                target="_blank"
                                                rel="noreferrer"
                                            >
                                                {item.title}
                                            </a>
                                            <span class="muted">{meta}</span>
                                        </li>
                                    }
                                })
                                .collect_view()}
                        </ul>
                    }
                        .into_any()
                }
                Some(Ok(WidgetData::Releases { items })) => {
                    view! {
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
                    }
                        .into_any()
                }
                Some(Ok(WidgetData::ServerStats(stats))) => {
                    view! { <ServerStatsView stats/> }.into_any()
                }
                Some(Err(err)) => view! { <p class="error">{err.to_string()}</p> }.into_any(),
            }}
        </section>
    }
}

#[component]
fn WeatherView(weather: WeatherData) -> impl IntoView {
    let details = format!(
        "{} · feels {:.0}° · wind {:.0} km/h{}",
        weather.location,
        weather.apparent_c,
        weather.wind_kmh,
        weather
            .humidity_pct
            .map(|h| format!(" · {h:.0}% humidity"))
            .unwrap_or_default(),
    );
    view! {
        <div class="weather">
            <div class="weather-now">
                <span class="weather-emoji">{weather_emoji(weather.weather_code)}</span>
                <span class="weather-temp">{format!("{:.0}°", weather.temperature_c)}</span>
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
                                <span>{weather_emoji(day.weather_code)}</span>
                                <span>{format!("{:.0}°", day.max_c)}</span>
                                <span class="muted">{format!("{:.0}°", day.min_c)}</span>
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

    view! {
        <div class="server-stats">
            <p class="muted">{head}</p>
            <Meter
                label="memory".to_string()
                used=stats.mem_used_bytes
                total=stats.mem_total_bytes
            />
            {stats
                .disks
                .into_iter()
                .map(|disk| {
                    view! { <Meter label=disk.mount used=disk.used_bytes total=disk.total_bytes/> }
                })
                .collect_view()}
        </div>
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

fn weather_emoji(code: i32) -> &'static str {
    match code {
        0 => "☀️",
        1 => "🌤️",
        2 => "⛅",
        3 => "☁️",
        45 | 48 => "🌫️",
        51..=57 => "🌦️",
        61..=67 | 80..=82 => "🌧️",
        71..=77 | 85 | 86 => "🌨️",
        95..=99 => "⛈️",
        _ => "🌡️",
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
        <section class="widget">
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
