//! Dedicated calendar page: month view over every calendar of the signed-in
//! user (local + ICS feeds), event creation/editing on local calendars, and
//! calendar management. Requires a session; logged off it shows a sign-in
//! prompt instead (the rest of the app stays usable).

use std::collections::HashMap;

use chaos_domain::{
    Calendar, CalendarEvent, CalendarKind, CalendarRequest, EventQuery, EventRequest,
};
use chrono::{DateTime, Datelike, Days, Duration, Local, NaiveDate, NaiveTime, TimeZone, Utc};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use uuid::Uuid;

use crate::components::Modal;
use crate::{use_client, use_session};

#[component]
pub fn CalendarPage() -> impl IntoView {
    let session = use_session();

    view! {
        {move || match session.0.get() {
            Some(_) => view! { <CalendarView/> }.into_any(),
            None => {
                view! {
                    <div class="calendar-signin muted">
                        <p>"Calendars are per user — sign in to see yours."</p>
                        <A href="/login">"Sign in"</A>
                    </div>
                }
                    .into_any()
            }
        }}
    }
}

/// What the event dialog opens with: an empty draft on some day, or an
/// existing local event to edit.
#[derive(Clone)]
struct EventDraft {
    id: Option<Uuid>,
    calendar_id: Option<Uuid>,
    title: String,
    location: String,
    description: String,
    all_day: bool,
    start_date: NaiveDate,
    start_time: NaiveTime,
    end_date: NaiveDate,
    end_time: NaiveTime,
}

impl EventDraft {
    fn new_on(day: NaiveDate) -> Self {
        Self {
            id: None,
            calendar_id: None,
            title: String::new(),
            location: String::new(),
            description: String::new(),
            all_day: false,
            start_date: day,
            start_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            end_date: day,
            end_time: NaiveTime::from_hms_opt(10, 0, 0).unwrap(),
        }
    }

    fn edit(event: &CalendarEvent) -> Self {
        let starts = event.starts_at.with_timezone(&Local);
        let ends = event.ends_at.with_timezone(&Local);
        // All-day events carry symbolic UTC dates (see `covers`).
        let (start_date, end_date) = if event.all_day {
            (
                event.starts_at.date_naive(),
                (event.ends_at - Duration::seconds(1)).date_naive(),
            )
        } else {
            (starts.date_naive(), ends.date_naive())
        };
        Self {
            id: event.id,
            calendar_id: Some(event.calendar_id),
            title: event.title.clone(),
            location: event.location.clone().unwrap_or_default(),
            description: event.description.clone().unwrap_or_default(),
            all_day: event.all_day,
            start_date,
            start_time: starts.time(),
            end_date,
            end_time: ends.time(),
        }
    }
}

#[component]
fn CalendarView() -> impl IntoView {
    let client = use_client();
    let today = Local::now().date_naive();

    let month = RwSignal::new((today.year(), today.month()));
    let selected = RwSignal::new(Some(today));
    let version = RwSignal::new(0u32);
    let dialog = RwSignal::new(None::<EventDraft>);
    let manage = RwSignal::new(false);

    let events = LocalResource::new({
        let client = client.clone();
        move || {
            version.track();
            let (start, end) = grid_utc_range(month.get());
            let client = client.clone();
            async move { client.calendar_events(&EventQuery { start, end }).await }
        }
    });
    let calendars = LocalResource::new({
        let client = client.clone();
        move || {
            version.track();
            let client = client.clone();
            async move { client.list_calendars().await }
        }
    });

    let shift = move |delta: i32| {
        month.update(|(year, m)| {
            let total = *year * 12 + (*m as i32 - 1) + delta;
            *year = total.div_euclid(12);
            *m = (total.rem_euclid(12) + 1) as u32;
        });
    };

    let local_calendars = Signal::derive(move || {
        calendars
            .get()
            .and_then(|r| r.ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.kind == CalendarKind::Local)
            .collect::<Vec<_>>()
    });

    view! {
        <div class="calendar-page">
            <div class="calendar-toolbar">
                <h2>
                    {move || {
                        let (year, m) = month.get();
                        NaiveDate::from_ymd_opt(year, m, 1)
                            .map(|d| d.format("%B %Y").to_string())
                            .unwrap_or_default()
                    }}
                </h2>
                <div class="calendar-nav">
                    <button title="Previous month" on:click=move |_| shift(-1)>"‹"</button>
                    <button
                        title="Today"
                        on:click=move |_| {
                            month.set((today.year(), today.month()));
                            selected.set(Some(today));
                        }
                    >
                        "Today"
                    </button>
                    <button title="Next month" on:click=move |_| shift(1)>"›"</button>
                </div>
                <div class="calendar-actions">
                    <button
                        title="Refetch calendar feeds"
                        on:click={
                            let client = client.clone();
                            move |_| {
                                let client = client.clone();
                                spawn_local(async move {
                                    if client.refresh_calendars().await.is_ok() {
                                        version.update(|n| *n += 1);
                                    }
                                });
                            }
                        }
                    >
                        "↻"
                    </button>
                    <button
                        class="primary"
                        on:click=move |_| {
                            dialog.set(Some(EventDraft::new_on(selected.get().unwrap_or(today))));
                        }
                    >
                        "New event"
                    </button>
                    <button on:click=move |_| manage.set(true)>"Calendars"</button>
                </div>
            </div>

            {move || match events.get() {
                None => view! { <p class="muted">"Loading events…"</p> }.into_any(),
                Some(Err(err)) if err.is_unauthorized() => {
                    view! {
                        <div class="calendar-signin muted">
                            <p>"Session expired."</p>
                            <A href="/login">"Sign in again"</A>
                        </div>
                    }
                        .into_any()
                }
                Some(Err(err)) => view! { <p class="error">{err.to_string()}</p> }.into_any(),
                Some(Ok(list)) => {
                    view! { <MonthGrid month=month.get() today selected events=list/> }.into_any()
                }
            }}

            {move || {
                selected
                    .get()
                    .map(|day| {
                        let day_events = events
                            .get()
                            .and_then(|r| r.ok())
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|e| covers(e, day))
                            .collect::<Vec<_>>();
                        view! { <DayPanel day events=day_events dialog version/> }
                    })
            }}
        </div>

        {move || {
            dialog
                .get()
                .map(|draft| {
                    view! {
                        <EventDialog
                            draft
                            calendars=local_calendars
                            on_done=Callback::new(move |changed: bool| {
                                dialog.set(None);
                                if changed {
                                    version.update(|n| *n += 1);
                                }
                            })
                        />
                    }
                })
        }}
        {move || {
            manage
                .get()
                .then(|| {
                    let all = calendars.get().and_then(|r| r.ok()).unwrap_or_default();
                    view! {
                        <CalendarsDialog
                            calendars=all
                            on_done=Callback::new(move |changed: bool| {
                                manage.set(false);
                                if changed {
                                    version.update(|n| *n += 1);
                                }
                            })
                        />
                    }
                })
        }}
    }
}

/// UTC range covering the 6-week grid shown for (year, month), in local time.
fn grid_utc_range((year, month): (i32, u32)) -> (DateTime<Utc>, DateTime<Utc>) {
    let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap_or_default();
    let start = first - Days::new(first.weekday().num_days_from_monday() as u64);
    (local_midnight(start), local_midnight(start + Days::new(42)))
}

fn local_midnight(date: NaiveDate) -> DateTime<Utc> {
    let naive = date.and_hms_opt(0, 0, 0).expect("midnight exists");
    Local
        .from_local_datetime(&naive)
        .single()
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|| Utc.from_utc_datetime(&naive))
}

fn local_to_utc(date: NaiveDate, time: NaiveTime) -> DateTime<Utc> {
    let naive = date.and_time(time);
    Local
        .from_local_datetime(&naive)
        .single()
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|| Utc.from_utc_datetime(&naive))
}

/// Does the event overlap the given local date? All-day events are
/// date-based, stored as symbolic UTC midnights (both ours and ICS feeds'),
/// so they must be compared as UTC dates — going through the local zone
/// would spill them into a neighbouring day.
fn covers(event: &CalendarEvent, day: NaiveDate) -> bool {
    // Exclusive end: subtract a second so a midnight end doesn't spill into
    // the next day — but clamp so a zero-duration event (ends_at ==
    // starts_at) still occupies its start day instead of an empty range.
    let end_at = (event.ends_at - Duration::seconds(1)).max(event.starts_at);
    let (start, end) = if event.all_day {
        (event.starts_at.date_naive(), end_at.date_naive())
    } else {
        (
            event.starts_at.with_timezone(&Local).date_naive(),
            end_at.with_timezone(&Local).date_naive(),
        )
    };
    start <= day && day <= end
}

/// Symbolic date for all-day events (see [`covers`]).
fn utc_midnight(date: NaiveDate) -> DateTime<Utc> {
    Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).expect("midnight exists"))
}

#[component]
fn MonthGrid(
    month: (i32, u32),
    today: NaiveDate,
    selected: RwSignal<Option<NaiveDate>>,
    events: Vec<CalendarEvent>,
) -> impl IntoView {
    let mut by_day: HashMap<NaiveDate, Vec<CalendarEvent>> = HashMap::new();
    let first = NaiveDate::from_ymd_opt(month.0, month.1, 1).unwrap_or_default();
    let start = first - Days::new(first.weekday().num_days_from_monday() as u64);
    for i in 0..42u64 {
        let day = start + Days::new(i);
        by_day.insert(
            day,
            events.iter().filter(|e| covers(e, day)).cloned().collect(),
        );
    }

    view! {
        <div class="calendar-grid calendar-grid-page">
            {["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"]
                .into_iter()
                .map(|day| view! { <span class="calendar-weekday muted">{day}</span> })
                .collect_view()}
            {(0..42u64)
                .map(|i| {
                    let day = start + Days::new(i);
                    let day_events = by_day.remove(&day).unwrap_or_default();
                    let mut class = String::from("calendar-day");
                    if day.month() != month.1 {
                        class.push_str(" other");
                    }
                    if day == today {
                        class.push_str(" today");
                    }
                    let shown: Vec<_> = day_events.iter().take(3).cloned().collect();
                    let more = day_events.len().saturating_sub(3);
                    view! {
                        <div
                            class=move || {
                                let mut c = class.clone();
                                if selected.get() == Some(day) {
                                    c.push_str(" selected");
                                }
                                c
                            }
                            on:click=move |_| selected.set(Some(day))
                        >
                            <span class="calendar-day-num">{day.day()}</span>
                            {shown
                                .into_iter()
                                .map(|event| {
                                    let style = event
                                        .color
                                        .as_ref()
                                        .map(|c| format!("border-left-color: {c}"))
                                        .unwrap_or_default();
                                    view! {
                                        <span class="event-chip" style=style title=event.title.clone()>
                                            {event.title.clone()}
                                        </span>
                                    }
                                })
                                .collect_view()}
                            {(more > 0).then(|| view! { <span class="muted event-more">{format!("+{more}")}</span> })}
                        </div>
                    }
                })
                .collect_view()}
        </div>
    }
}

#[component]
fn DayPanel(
    day: NaiveDate,
    events: Vec<CalendarEvent>,
    dialog: RwSignal<Option<EventDraft>>,
    version: RwSignal<u32>,
) -> impl IntoView {
    let client = use_client();

    view! {
        <div class="day-panel">
            <h3>{day.format("%A %e %B").to_string()}</h3>
            {if events.is_empty() {
                view! { <p class="muted">"No events."</p> }.into_any()
            } else {
                view! {
                    <ul class="day-events">
                        {events
                            .into_iter()
                            .map(|event| {
                                let time = if event.all_day {
                                    "all day".to_string()
                                } else {
                                    format!(
                                        "{} – {}",
                                        event.starts_at.with_timezone(&Local).format("%H:%M"),
                                        event.ends_at.with_timezone(&Local).format("%H:%M"),
                                    )
                                };
                                let meta = [Some(event.calendar_name.clone()), event.location.clone()]
                                    .into_iter()
                                    .flatten()
                                    .collect::<Vec<_>>()
                                    .join(" · ");
                                let editable = event.id;
                                let edit_event = event.clone();
                                let client = client.clone();
                                let description = event
                                    .description
                                    .clone()
                                    .filter(|d| !d.trim().is_empty())
                                    .map(|d| view! { <p class="event-desc muted">{d}</p> });
                                view! {
                                    <li>
                                        <span class="event-time muted">{time}</span>
                                        <span class="event-title">{event.title.clone()}</span>
                                        <span class="muted event-meta">{meta}</span>
                                        {description}
                                        {editable
                                            .map(|id| {
                                                view! {
                                                    <span class="unit-actions">
                                                        <button
                                                            class="unit-btn"
                                                            title="Edit"
                                                            on:click=move |_| {
                                                                dialog.set(Some(EventDraft::edit(&edit_event)));
                                                            }
                                                        >
                                                            "✎"
                                                        </button>
                                                        <button
                                                            class="unit-btn"
                                                            title="Delete"
                                                            on:click=move |_| {
                                                                let client = client.clone();
                                                                spawn_local(async move {
                                                                    if client.delete_event(id).await.is_ok() {
                                                                        version.update(|n| *n += 1);
                                                                    }
                                                                });
                                                            }
                                                        >
                                                            "✕"
                                                        </button>
                                                    </span>
                                                }
                                            })}
                                    </li>
                                }
                            })
                            .collect_view()}
                    </ul>
                }
                    .into_any()
            }}
        </div>
    }
}

#[component]
fn EventDialog(
    draft: EventDraft,
    calendars: Signal<Vec<Calendar>>,
    on_done: Callback<bool>,
) -> impl IntoView {
    let client = use_client();
    let editing = draft.id;

    let title = RwSignal::new(draft.title.clone());
    let location = RwSignal::new(draft.location.clone());
    let description = RwSignal::new(draft.description.clone());
    let all_day = RwSignal::new(draft.all_day);
    let start_date = RwSignal::new(draft.start_date.format("%Y-%m-%d").to_string());
    let start_time = RwSignal::new(draft.start_time.format("%H:%M").to_string());
    let end_date = RwSignal::new(draft.end_date.format("%Y-%m-%d").to_string());
    let end_time = RwSignal::new(draft.end_time.format("%H:%M").to_string());
    let calendar_id = RwSignal::new(
        draft
            .calendar_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
    );
    let error = RwSignal::new(None::<String>);

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();

        let chosen_calendar = calendar_id
            .get_untracked()
            .parse::<Uuid>()
            .ok()
            .or_else(|| calendars.get_untracked().first().map(|c| c.id));
        let Some(calendar_id) = chosen_calendar else {
            error.set(Some(
                "Create a local calendar first (Calendars button)".into(),
            ));
            return;
        };

        let (Ok(sd), Ok(ed)) = (
            NaiveDate::parse_from_str(&start_date.get_untracked(), "%Y-%m-%d"),
            NaiveDate::parse_from_str(&end_date.get_untracked(), "%Y-%m-%d"),
        ) else {
            error.set(Some("Invalid date".into()));
            return;
        };

        let (starts_at, ends_at) = if all_day.get_untracked() {
            (utc_midnight(sd), utc_midnight(ed + Days::new(1)))
        } else {
            let (Ok(st), Ok(et)) = (
                NaiveTime::parse_from_str(&start_time.get_untracked(), "%H:%M"),
                NaiveTime::parse_from_str(&end_time.get_untracked(), "%H:%M"),
            ) else {
                error.set(Some("Invalid time".into()));
                return;
            };
            (local_to_utc(sd, st), local_to_utc(ed, et))
        };

        let req = EventRequest {
            calendar_id,
            title: title.get_untracked(),
            description: Some(description.get_untracked()).filter(|s| !s.trim().is_empty()),
            location: Some(location.get_untracked()).filter(|s| !s.trim().is_empty()),
            starts_at,
            ends_at,
            all_day: all_day.get_untracked(),
        };
        let client = client.clone();
        spawn_local(async move {
            let result = match editing {
                Some(id) => client.update_event(id, &req).await,
                None => client.create_event(&req).await,
            };
            match result {
                Ok(_) => on_done.run(true),
                Err(err) => error.set(Some(err.to_string())),
            }
        });
    };

    view! {
        <Modal
            title=if editing.is_some() { "Edit event".to_string() } else { "New event".to_string() }
            on_close=Callback::new(move |()| on_done.run(false))
        >
            <form class="stack" on:submit=submit>
                <label>
                    "Title"
                    <input
                        type="text"
                        autofocus
                        prop:value=title
                        on:input=move |ev| title.set(event_target_value(&ev))
                    />
                </label>
                <label>
                    "Calendar"
                    <select on:change=move |ev| calendar_id.set(event_target_value(&ev))>
                        {move || {
                            calendars
                                .get()
                                .into_iter()
                                .map(|c| {
                                    let selected = calendar_id.get() == c.id.to_string();
                                    view! {
                                        <option value=c.id.to_string() selected=selected>
                                            {c.name}
                                        </option>
                                    }
                                })
                                .collect_view()
                        }}
                    </select>
                </label>
                <label class="row">
                    <input
                        type="checkbox"
                        prop:checked=all_day
                        on:change=move |ev| all_day.set(event_target_checked(&ev))
                    />
                    "All day"
                </label>
                <div class="row">
                    <label>
                        "Starts"
                        <input
                            type="date"
                            prop:value=start_date
                            on:input=move |ev| start_date.set(event_target_value(&ev))
                        />
                    </label>
                    {move || {
                        (!all_day.get())
                            .then(|| {
                                view! {
                                    <input
                                        type="time"
                                        prop:value=start_time
                                        on:input=move |ev| start_time.set(event_target_value(&ev))
                                    />
                                }
                            })
                    }}
                </div>
                <div class="row">
                    <label>
                        "Ends"
                        <input
                            type="date"
                            prop:value=end_date
                            on:input=move |ev| end_date.set(event_target_value(&ev))
                        />
                    </label>
                    {move || {
                        (!all_day.get())
                            .then(|| {
                                view! {
                                    <input
                                        type="time"
                                        prop:value=end_time
                                        on:input=move |ev| end_time.set(event_target_value(&ev))
                                    />
                                }
                            })
                    }}
                </div>
                <label>
                    "Location"
                    <input
                        type="text"
                        prop:value=location
                        on:input=move |ev| location.set(event_target_value(&ev))
                    />
                </label>
                <label>
                    "Description"
                    <textarea
                        prop:value=description
                        on:input=move |ev| description.set(event_target_value(&ev))
                    ></textarea>
                </label>
                {move || error.get().map(|err| view! { <p class="error">{err}</p> })}
                <div class="row end">
                    <button type="submit" class="primary">"Save"</button>
                </div>
            </form>
        </Modal>
    }
}

#[component]
fn CalendarsDialog(calendars: Vec<Calendar>, on_done: Callback<bool>) -> impl IntoView {
    let client = use_client();
    let changed = RwSignal::new(false);

    let name = RwSignal::new(String::new());
    let color = RwSignal::new(String::from("#7c9aff"));
    let is_feed = RwSignal::new(false);
    let ics_url = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    // Deleted ids, to hide rows without refetching mid-dialog.
    let removed = RwSignal::new(Vec::<Uuid>::new());

    let add = {
        let client = client.clone();
        move |ev: leptos::ev::SubmitEvent| {
            ev.prevent_default();
            let req = CalendarRequest {
                name: name.get_untracked(),
                color: Some(color.get_untracked()).filter(|s| !s.is_empty()),
                kind: if is_feed.get_untracked() {
                    CalendarKind::Ics
                } else {
                    CalendarKind::Local
                },
                ics_url: Some(ics_url.get_untracked()).filter(|s| !s.trim().is_empty()),
            };
            let client = client.clone();
            spawn_local(async move {
                match client.create_calendar(&req).await {
                    Ok(_) => {
                        changed.set(true);
                        on_done.run(true);
                    }
                    Err(err) => error.set(Some(err.to_string())),
                }
            });
        }
    };

    view! {
        <Modal
            title="Calendars".to_string()
            on_close=Callback::new(move |()| on_done.run(changed.get_untracked()))
        >
            <ul class="calendar-list">
                {calendars
                    .into_iter()
                    .map(|calendar| {
                        let id = calendar.id;
                        let client = client.clone();
                        let kind = match calendar.kind {
                            CalendarKind::Local => "local",
                            CalendarKind::Ics => "feed",
                        };
                        let swatch = calendar
                            .color
                            .clone()
                            .map(|c| format!("background: {c}"))
                            .unwrap_or_default();
                        view! {
                            <li style:display=move || {
                                if removed.get().contains(&id) { "none" } else { "" }
                            }>
                                <span class="calendar-swatch" style=swatch></span>
                                <span class="event-title">{calendar.name.clone()}</span>
                                <span class="muted">{kind}</span>
                                <button
                                    class="unit-btn"
                                    title="Delete calendar (and its events)"
                                    on:click=move |_| {
                                        let client = client.clone();
                                        spawn_local(async move {
                                            if client.delete_calendar(id).await.is_ok() {
                                                changed.set(true);
                                                removed.update(|r| r.push(id));
                                            }
                                        });
                                    }
                                >
                                    "✕"
                                </button>
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>

            <form class="stack" on:submit=add>
                <h4>"Add a calendar"</h4>
                <div class="row">
                    <label>
                        "Name"
                        <input
                            type="text"
                            prop:value=name
                            on:input=move |ev| name.set(event_target_value(&ev))
                        />
                    </label>
                    <label>
                        "Color"
                        <input
                            type="color"
                            prop:value=color
                            on:input=move |ev| color.set(event_target_value(&ev))
                        />
                    </label>
                </div>
                <label class="row">
                    <input
                        type="checkbox"
                        prop:checked=is_feed
                        on:change=move |ev| is_feed.set(event_target_checked(&ev))
                    />
                    "Subscribe to an ICS feed (read-only)"
                </label>
                {move || {
                    is_feed
                        .get()
                        .then(|| {
                            view! {
                                <label>
                                    "Feed URL (Google \"secret address\", Proton share link, any .ics)"
                                    <input
                                        type="url"
                                        prop:value=ics_url
                                        on:input=move |ev| ics_url.set(event_target_value(&ev))
                                    />
                                </label>
                            }
                        })
                }}
                {move || error.get().map(|err| view! { <p class="error">{err}</p> })}
                <div class="row end">
                    <button type="submit" class="primary">"Add"</button>
                </div>
            </form>
        </Modal>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Editing must load every event field into the draft: updates are
    /// full-replacement PUTs, so a field the draft drops is a field the
    /// save deletes.
    #[test]
    fn edit_carries_over_the_description() {
        let event = CalendarEvent {
            id: Some(Uuid::from_u128(1)),
            calendar_id: Uuid::from_u128(2),
            calendar_name: "Perso".into(),
            color: None,
            title: "Dentist".into(),
            description: Some("Bring the x-rays".into()),
            location: Some("12 rue des Lilas".into()),
            starts_at: Utc.with_ymd_and_hms(2026, 7, 11, 9, 0, 0).unwrap(),
            ends_at: Utc.with_ymd_and_hms(2026, 7, 11, 10, 0, 0).unwrap(),
            all_day: false,
        };

        let draft = EventDraft::edit(&event);

        assert_eq!(draft.title, "Dentist");
        assert_eq!(draft.location, "12 rue des Lilas");
        assert_eq!(
            draft.description, "Bring the x-rays",
            "editing an event must not blank its description"
        );
    }

    fn event(starts_at: DateTime<Utc>, ends_at: DateTime<Utc>, all_day: bool) -> CalendarEvent {
        CalendarEvent {
            id: None,
            calendar_id: Uuid::nil(),
            calendar_name: "test".into(),
            color: None,
            title: "standup".into(),
            description: None,
            location: None,
            starts_at,
            ends_at,
            all_day,
        }
    }

    #[test]
    fn covers_shows_a_zero_duration_all_day_event_on_its_start_day() {
        let start = Utc.with_ymd_and_hms(2026, 7, 11, 0, 0, 0).unwrap();
        let e = event(start, start, true);
        assert!(covers(&e, NaiveDate::from_ymd_opt(2026, 7, 11).unwrap()));
        assert!(!covers(&e, NaiveDate::from_ymd_opt(2026, 7, 12).unwrap()));
        assert!(!covers(&e, NaiveDate::from_ymd_opt(2026, 7, 10).unwrap()));
    }

    #[test]
    fn covers_shows_a_zero_duration_timed_event_on_its_local_start_day() {
        // Timezone-independent: compare against the start's own local date.
        let start = Utc.with_ymd_and_hms(2026, 7, 11, 9, 30, 0).unwrap();
        let e = event(start, start, false);
        let local_day = start.with_timezone(&Local).date_naive();
        assert!(covers(&e, local_day));
    }

    #[test]
    fn covers_still_excludes_the_exclusive_end_midnight() {
        // A one-day all-day event ends at the next UTC midnight, exclusively;
        // the -1s adjustment must keep it off the following day.
        let start = Utc.with_ymd_and_hms(2026, 7, 11, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2026, 7, 12, 0, 0, 0).unwrap();
        let e = event(start, end, true);
        assert!(covers(&e, NaiveDate::from_ymd_opt(2026, 7, 11).unwrap()));
        assert!(!covers(&e, NaiveDate::from_ymd_opt(2026, 7, 12).unwrap()));
    }
}
