//! `GET /api/v1/search`: the global quick-search (Ctrl-K in the UI).
//!
//! Aggregates config-defined services and bookmarks, stored links (the
//! existing LIKE + FTS5 query path), and — when the request carries a
//! session — the user's calendar events. Public like the links API; only
//! the events group is user-scoped (logged off → empty, never an error).

use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use chaos_domain::{
    BookmarkGroup, CalendarEvent, LinkQuery, SearchHit, SearchKind, SearchQuery, SearchResults,
    ServiceDef, Widget,
};
use chrono::{Duration, Utc};
use uuid::Uuid;

use crate::api::ApiError;
use crate::state::AppState;

/// Cap per result group, so one noisy section cannot drown the palette.
const GROUP_LIMIT: usize = 10;
/// Events are searched in a window of now ± this many days.
const EVENT_WINDOW_DAYS: i64 = 60;

pub async fn search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResults>, ApiError> {
    let q = query.q.trim();
    if q.is_empty() {
        return Ok(Json(SearchResults::default()));
    }
    let user_id = crate::auth::optional_user_id(&state, &headers).await;
    Ok(Json(aggregate(&state, user_id, q).await?))
}

async fn aggregate(
    state: &AppState,
    user_id: Option<Uuid>,
    q: &str,
) -> Result<SearchResults, ApiError> {
    let services = filter_services(&state.config.services, q);
    let bookmarks = filter_bookmarks(&bookmark_groups(state), q);

    let links = state
        .db
        .list_links(&LinkQuery {
            q: Some(q.to_string()),
            limit: Some(GROUP_LIMIT as u32),
            ..Default::default()
        })
        .await?
        .items
        .into_iter()
        .map(|link| SearchHit {
            kind: SearchKind::Link,
            title: link.title,
            subtitle: link.url.host_str().map(String::from),
            url: Some(link.url),
        })
        .collect();

    // Logged off → no events, matching the calendar API's auth semantics.
    // merged_events hits the ICS feed cache (10min TTL); on a cold miss a
    // debounced search can trigger real feed fetches, and StaleCache has no
    // single-flight yet (see cache.rs TODO) — acceptable while feeds are
    // few, revisit before adding heavier upstreams.
    let events = match user_id {
        Some(user_id) => {
            let now = Utc::now();
            let merged = super::calendar::merged_events(
                state,
                user_id,
                now - Duration::days(EVENT_WINDOW_DAYS),
                now + Duration::days(EVENT_WINDOW_DAYS),
            )
            .await?;
            filter_events(&merged, q, now)
        }
        None => Vec::new(),
    };

    Ok(SearchResults {
        services,
        bookmarks,
        links,
        events,
    })
}

fn matches(haystack: &str, q: &str) -> bool {
    haystack.to_lowercase().contains(&q.to_lowercase())
}

fn filter_services(services: &[ServiceDef], q: &str) -> Vec<SearchHit> {
    services
        .iter()
        .filter(|s| matches(&s.title, q) || matches(&s.id, q))
        .take(GROUP_LIMIT)
        .map(|s| SearchHit {
            kind: SearchKind::Service,
            title: s.title.clone(),
            subtitle: s.url.host_str().map(String::from),
            url: Some(s.url.clone()),
        })
        .collect()
}

/// Bookmark groups can live at the top level and/or inside `bookmarks`
/// widgets in the column layout; search both.
fn bookmark_groups(state: &AppState) -> Vec<&BookmarkGroup> {
    let mut groups: Vec<&BookmarkGroup> = state.config.bookmarks.iter().collect();
    for column in &state.config.columns {
        for widget in &column.widgets {
            if let Widget::Bookmarks { groups: g } = widget {
                groups.extend(g.iter());
            }
        }
    }
    groups
}

fn filter_bookmarks(groups: &[&BookmarkGroup], q: &str) -> Vec<SearchHit> {
    groups
        .iter()
        .flat_map(|group| {
            group
                .links
                .iter()
                .filter(|b| matches(&b.title, q))
                .map(|b| SearchHit {
                    kind: SearchKind::Bookmark,
                    title: b.title.clone(),
                    subtitle: Some(group.title.clone()),
                    url: Some(b.url.clone()),
                })
        })
        .take(GROUP_LIMIT)
        .collect()
}

fn filter_events(events: &[CalendarEvent], q: &str, now: chrono::DateTime<Utc>) -> Vec<SearchHit> {
    // merged_events sorts ascending, so a recurring event would fill the
    // group with weeks-old occurrences; the upcoming ones come first here.
    let hits = |future: bool| {
        events
            .iter()
            .filter(move |e| (e.starts_at >= now) == future && matches(&e.title, q))
    };
    hits(true)
        .chain(hits(false).rev())
        .take(GROUP_LIMIT)
        .map(|e| SearchHit {
            kind: SearchKind::Event,
            title: e.title.clone(),
            subtitle: Some(format!(
                "{} · {}",
                e.starts_at.format("%a %-d %b %H:%M"),
                e.calendar_name
            )),
            url: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chaos_domain::{Bookmark, CalendarKind, CalendarRequest, CreateLinkRequest, EventRequest};

    use crate::config::Config;
    use crate::db::Db;

    #[tokio::test]
    async fn aggregate_searches_config_links_and_only_the_users_events() {
        let db = Db::in_memory().await.unwrap();
        let user = db.create_user("tibo", "Tibo", "phc").await.unwrap();
        let cal = db
            .create_calendar(
                user.id,
                &CalendarRequest {
                    name: "Perso".into(),
                    color: None,
                    kind: CalendarKind::Local,
                    ics_url: None,
                },
            )
            .await
            .unwrap();
        db.create_event(
            user.id,
            &EventRequest {
                calendar_id: cal.id,
                title: "Dentist appointment".into(),
                description: None,
                location: None,
                starts_at: Utc::now() + Duration::days(3),
                ends_at: Utc::now() + Duration::days(3) + Duration::hours(1),
                all_day: false,
            },
        )
        .await
        .unwrap();
        db.create_link(
            &CreateLinkRequest {
                url: "https://example.com/dentures".parse().unwrap(),
                title: Some("Denture care guide".into()),
                description: None,
                collection_id: None,
                tags: vec![],
            },
            false,
            None,
        )
        .await
        .unwrap();

        let config = Config {
            services: vec![ServiceDef {
                id: "jellyfin".into(),
                title: "Jellyfin".into(),
                url: "http://zeus:8096".parse().unwrap(),
                icon: None,
                check_url: None,
                unit: None,
            }],
            bookmarks: vec![BookmarkGroup {
                title: "Main".into(),
                links: vec![Bookmark {
                    title: "Denpa News".into(),
                    url: "https://denpa.example.com".parse().unwrap(),
                    icon: None,
                    android_package: None,
                }],
            }],
            ..Config::default()
        };
        let state = crate::state::AppState::new(config, db).unwrap();

        // Case-insensitive substring, hits in three groups at once.
        let results = aggregate(&state, Some(user.id), "DEN").await.unwrap();
        assert!(results.services.is_empty());
        assert_eq!(results.bookmarks.len(), 1);
        assert_eq!(results.bookmarks[0].subtitle.as_deref(), Some("Main"));
        assert_eq!(results.links.len(), 1);
        assert_eq!(results.links[0].kind, SearchKind::Link);
        assert_eq!(results.events.len(), 1);
        assert_eq!(results.events[0].kind, SearchKind::Event);
        assert!(
            results.events[0].url.is_none(),
            "events route via /calendar"
        );

        let jelly = aggregate(&state, Some(user.id), "jelly").await.unwrap();
        assert_eq!(jelly.services.len(), 1);
        assert_eq!(
            jelly.services[0].url.as_ref().unwrap().as_str(),
            "http://zeus:8096/"
        );

        // Logged off: events stay private, everything else is public.
        let anon = aggregate(&state, None, "den").await.unwrap();
        assert!(anon.events.is_empty());
        assert_eq!(anon.links.len(), 1);
        assert_eq!(anon.bookmarks.len(), 1);
    }

    #[test]
    fn group_filters_cap_and_match_case_insensitively() {
        let services: Vec<ServiceDef> = (0..15)
            .map(|i| ServiceDef {
                id: format!("svc-{i}"),
                title: format!("Service {i}"),
                url: "http://zeus:1234".parse().unwrap(),
                icon: None,
                check_url: None,
                unit: None,
            })
            .collect();
        assert_eq!(filter_services(&services, "SERVICE").len(), GROUP_LIMIT);
        assert_eq!(filter_services(&services, "svc-14").len(), 1);
        assert!(filter_services(&services, "nope").is_empty());
    }

    #[test]
    fn events_prefer_upcoming_occurrences_over_stale_past_ones() {
        use chrono::{Duration, TimeZone};
        let now = Utc.with_ymd_and_hms(2026, 7, 11, 12, 0, 0).unwrap();
        // A recurring event: many past occurrences, a few upcoming — the
        // group must not fill with the old ones (merged_events sorts
        // ascending, so the naive take() would).
        let events: Vec<CalendarEvent> = (-20..3)
            .map(|d| CalendarEvent {
                id: None,
                calendar_id: uuid::Uuid::nil(),
                calendar_name: "perso".into(),
                color: None,
                title: "Standup".into(),
                description: None,
                location: None,
                starts_at: now + Duration::days(d),
                ends_at: now + Duration::days(d) + Duration::hours(1),
                all_day: false,
            })
            .collect();

        let hits = filter_events(&events, "standup", now);
        assert_eq!(hits.len(), GROUP_LIMIT);
        // The three upcoming occurrences (d = 0, 1, 2) lead the group,
        // then the most recent past ones.
        assert!(
            hits[0]
                .subtitle
                .as_deref()
                .unwrap()
                .starts_with("Sat 11 Jul")
        );
        assert!(
            hits[1]
                .subtitle
                .as_deref()
                .unwrap()
                .starts_with("Sun 12 Jul")
        );
        assert!(
            hits[2]
                .subtitle
                .as_deref()
                .unwrap()
                .starts_with("Mon 13 Jul")
        );
        assert!(
            hits[3]
                .subtitle
                .as_deref()
                .unwrap()
                .starts_with("Fri 10 Jul")
        );
    }
}
