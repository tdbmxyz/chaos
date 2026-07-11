//! ICS feed subscriptions: fetch, parse and cache external calendars
//! (Google Calendar secret address, Proton Calendar share link, any .ics).
//!
//! The parsed feed is cached per calendar; recurrence expansion happens per
//! query because it depends on the requested range. Feeds are read-only by
//! design — writing goes to local calendars (CalDAV two-way sync would be
//! its own project).

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Days, NaiveDate, NaiveDateTime, TimeZone, Utc};
use uuid::Uuid;

use crate::cache::StaleCache;

const TTL: Duration = Duration::from_secs(600);
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
/// Cap on cached feeds; keys are the user's calendar ids, so this is
/// effectively "how many feed calendars one server realistically has".
const MAX_FEEDS: usize = 128;
/// Cap on expanded occurrences per event and range, against pathological
/// rules; a month view never legitimately needs more.
const MAX_OCCURRENCES: u16 = 400;

/// One event as parsed from a feed, before recurrence expansion.
#[derive(Debug, Clone)]
struct RawEvent {
    title: String,
    description: Option<String>,
    location: Option<String>,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    all_day: bool,
    /// Reconstructed `DTSTART`/`RRULE`/`EXDATE`/`RDATE` block for the rrule
    /// crate, present only for recurring events.
    rrule_block: Option<String>,
}

/// One concrete occurrence inside a queried range.
#[derive(Debug, Clone)]
pub struct FeedEvent {
    pub title: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub all_day: bool,
}

pub struct FeedCache {
    entries: StaleCache<Uuid, Arc<Vec<RawEvent>>>,
    http: reqwest::Client,
}

impl Default for FeedCache {
    fn default() -> Self {
        Self {
            entries: StaleCache::new(MAX_FEEDS),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .user_agent(concat!("chaos/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("building ics http client"),
        }
    }
}

impl FeedCache {
    /// Occurrences of the feed overlapping [start, end).
    pub async fn events(
        &self,
        calendar_id: Uuid,
        url: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<FeedEvent>, String> {
        let raw = self.raw_events(calendar_id, url).await?;
        let mut out = Vec::new();
        for event in raw.iter() {
            expand(event, start, end, &mut out);
        }
        Ok(out)
    }

    async fn raw_events(&self, calendar_id: Uuid, url: &str) -> Result<Arc<Vec<RawEvent>>, String> {
        if let Some(events) = self.entries.get_fresh(&calendar_id, TTL).await {
            return Ok(events);
        }

        match self.fetch(url).await {
            Ok(events) => {
                let events = Arc::new(events);
                self.entries.insert(calendar_id, events.clone()).await;
                Ok(events)
            }
            Err(reason) => {
                // Serve the stale copy if we have one.
                if let Some(events) = self.entries.get_stale(&calendar_id).await {
                    tracing::warn!(%calendar_id, reason, "ics refresh failed, serving stale feed");
                    return Ok(events);
                }
                Err(reason)
            }
        }
    }

    async fn fetch(&self, url: &str) -> Result<Vec<RawEvent>, String> {
        let body = crate::http_util::get_body_capped(&self.http, url, MAX_BODY_BYTES).await?;
        parse(&body)
    }

    /// Drop a cached feed (after its calendar is edited or deleted).
    pub async fn invalidate(&self, calendar_id: Uuid) {
        self.entries.remove(&calendar_id).await;
    }
}

fn parse(body: &[u8]) -> Result<Vec<RawEvent>, String> {
    let mut events = Vec::new();
    for calendar in ical::IcalParser::new(body) {
        let calendar = calendar.map_err(|e| e.to_string())?;
        for vevent in calendar.events {
            match raw_event(&vevent) {
                Some(event) => events.push(event),
                None => tracing::debug!("skipping VEVENT without usable DTSTART"),
            }
        }
    }
    Ok(events)
}

fn raw_event(vevent: &ical::parser::ical::component::IcalEvent) -> Option<RawEvent> {
    let prop = |name: &str| vevent.properties.iter().find(|p| p.name == name);
    let value = |name: &str| prop(name).and_then(|p| p.value.clone());

    let dtstart = prop("DTSTART")?;
    let (starts_at, all_day) = parse_datetime(dtstart)?;

    let ends_at = prop("DTEND")
        .and_then(parse_datetime)
        .map(|(dt, _)| dt)
        .unwrap_or_else(|| default_end(starts_at, all_day));

    // Recurring events keep their raw timing lines so the rrule crate can
    // interpret them (incl. timezones and EXDATE) exactly as written.
    let rrule_block = prop("RRULE").map(|_| {
        ["DTSTART", "RRULE", "EXDATE", "RDATE"]
            .iter()
            .flat_map(|name| vevent.properties.iter().filter(move |p| p.name == *name))
            .map(raw_line)
            .collect::<Vec<_>>()
            .join("\n")
    });

    Some(RawEvent {
        title: value("SUMMARY").unwrap_or_else(|| "(untitled)".into()),
        description: value("DESCRIPTION").filter(|s| !s.is_empty()),
        location: value("LOCATION").filter(|s| !s.is_empty()),
        starts_at,
        ends_at,
        all_day,
        rrule_block,
    })
}

/// Reconstruct a property as an iCalendar content line.
fn raw_line(prop: &ical::property::Property) -> String {
    let mut line = prop.name.clone();
    if let Some(params) = &prop.params {
        for (key, values) in params {
            line.push(';');
            line.push_str(key);
            line.push('=');
            line.push_str(&values.join(","));
        }
    }
    line.push(':');
    line.push_str(prop.value.as_deref().unwrap_or(""));
    line
}

/// DTSTART/DTEND in their three shapes: DATE (all-day), UTC datetime,
/// and local datetime with an optional TZID parameter.
fn parse_datetime(prop: &ical::property::Property) -> Option<(DateTime<Utc>, bool)> {
    let raw = prop.value.as_deref()?.trim();

    let is_date = raw.len() == 8
        || prop.params.as_ref().is_some_and(|params| {
            params
                .iter()
                .any(|(k, v)| k == "VALUE" && v.iter().any(|s| s == "DATE"))
        });
    if is_date {
        // Known limitation: all-day events are pinned to UTC midnight, so
        // viewers in negative-UTC-offset zones see them start a day early.
        // Fixing this means plumbing a display timezone through the whole
        // calendar API — deliberately out of scope here.
        let date = raw
            .get(..8)
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y%m%d").ok())?;
        return Some((Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0)?), true));
    }

    if let Some(stripped) = raw.strip_suffix('Z') {
        let naive = NaiveDateTime::parse_from_str(stripped, "%Y%m%dT%H%M%S").ok()?;
        return Some((Utc.from_utc_datetime(&naive), false));
    }

    let naive = NaiveDateTime::parse_from_str(raw, "%Y%m%dT%H%M%S").ok()?;
    let tzid = prop.params.as_ref().and_then(|params| {
        params
            .iter()
            .find(|(k, _)| k == "TZID")
            .and_then(|(_, v)| v.first().cloned())
    });
    let utc = match tzid.and_then(|id| id.parse::<chrono_tz::Tz>().ok()) {
        Some(tz) => tz.from_local_datetime(&naive).single()?.with_timezone(&Utc),
        // Floating time: interpret as UTC rather than guessing a zone.
        None => Utc.from_utc_datetime(&naive),
    };
    Some((utc, false))
}

fn default_end(start: DateTime<Utc>, all_day: bool) -> DateTime<Utc> {
    if all_day {
        start + Days::new(1)
    } else {
        start + chrono::Duration::hours(1)
    }
}

/// Push the occurrences of one raw event that overlap [start, end).
fn expand(event: &RawEvent, start: DateTime<Utc>, end: DateTime<Utc>, out: &mut Vec<FeedEvent>) {
    let duration = event.ends_at - event.starts_at;

    let Some(block) = &event.rrule_block else {
        if event.starts_at < end && event.ends_at > start {
            out.push(occurrence(event, event.starts_at, duration));
        }
        return;
    };

    let set: rrule::RRuleSet = match block.parse() {
        Ok(set) => set,
        Err(err) => {
            // Unparseable rule: degrade to the first occurrence.
            tracing::debug!(error = %err, title = event.title, "unsupported RRULE, showing base event only");
            if event.starts_at < end && event.ends_at > start {
                out.push(occurrence(event, event.starts_at, duration));
            }
            return;
        }
    };

    // Widen the window backwards so an occurrence that started before the
    // range but overlaps into it is still found.
    let window_start = (start - duration).with_timezone(&rrule::Tz::UTC);
    let window_end = end.with_timezone(&rrule::Tz::UTC);
    let result = set
        .after(window_start)
        .before(window_end)
        .all(MAX_OCCURRENCES);
    if result.limited {
        tracing::debug!(
            title = event.title,
            limit = MAX_OCCURRENCES,
            "recurrence expansion truncated at MAX_OCCURRENCES"
        );
    }
    for date in result.dates {
        let starts_at = date.with_timezone(&Utc);
        if starts_at < end && starts_at + duration > start {
            out.push(occurrence(event, starts_at, duration));
        }
    }
}

fn occurrence(event: &RawEvent, starts_at: DateTime<Utc>, duration: chrono::Duration) -> FeedEvent {
    FeedEvent {
        title: event.title.clone(),
        description: event.description.clone(),
        location: event.location.clone(),
        starts_at,
        ends_at: starts_at + duration,
        all_day: event.all_day,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nSUMMARY:Bastille Day\r\nDTSTART;VALUE=DATE:20260714\r\nDTEND;VALUE=DATE:20260715\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nSUMMARY:Standup\r\nLOCATION:Meet\r\nDESCRIPTION:Daily sync\r\nDTSTART;TZID=Europe/Paris:20260701T093000\r\nDTEND;TZID=Europe/Paris:20260701T094500\r\nRRULE:FREQ=WEEKLY;BYDAY=WE\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nSUMMARY:One-off\r\nDTSTART:20260710T120000Z\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";

    fn range(from: (i32, u32, u32), to: (i32, u32, u32)) -> (DateTime<Utc>, DateTime<Utc>) {
        (
            Utc.with_ymd_and_hms(from.0, from.1, from.2, 0, 0, 0)
                .unwrap(),
            Utc.with_ymd_and_hms(to.0, to.1, to.2, 0, 0, 0).unwrap(),
        )
    }

    #[test]
    fn parses_and_expands_a_feed() {
        let raw = parse(SAMPLE.as_bytes()).expect("parse");
        assert_eq!(raw.len(), 3);

        let (start, end) = range((2026, 7, 1), (2026, 8, 1));
        let mut out = Vec::new();
        for event in &raw {
            expand(event, start, end, &mut out);
        }

        let holidays: Vec<_> = out.iter().filter(|e| e.title == "Bastille Day").collect();
        assert_eq!(holidays.len(), 1);
        assert!(holidays[0].all_day);

        // Weekly standup: July 2026 has five Wednesdays (1, 8, 15, 22, 29),
        // 09:30 Paris = 07:30 UTC in summer.
        let standups: Vec<_> = out.iter().filter(|e| e.title == "Standup").collect();
        assert_eq!(standups.len(), 5);
        assert_eq!(standups[0].starts_at.format("%H:%M").to_string(), "07:30");
        assert_eq!(
            (standups[0].ends_at - standups[0].starts_at).num_minutes(),
            15
        );

        // Missing DTEND defaults to one hour.
        let one_off = out.iter().find(|e| e.title == "One-off").expect("one-off");
        assert_eq!((one_off.ends_at - one_off.starts_at).num_hours(), 1);
    }

    #[test]
    fn range_filters_out_of_window_occurrences() {
        let raw = parse(SAMPLE.as_bytes()).expect("parse");
        let (start, end) = range((2026, 9, 1), (2026, 9, 8));
        let mut out = Vec::new();
        for event in &raw {
            expand(event, start, end, &mut out);
        }
        // Only the standup recurs into September (Wed Sep 2).
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "Standup");
        assert_eq!(out[0].description.as_deref(), Some("Daily sync"));
    }

    #[test]
    fn parse_datetime_survives_multibyte_date_values() {
        // 9 bytes: byte index 8 falls inside the two-byte 'é', so a naive
        // `&raw[..8]` slice panics. Must return None instead.
        let prop = ical::property::Property {
            name: "DTSTART".into(),
            params: Some(vec![("VALUE".into(), vec!["DATE".into()])]),
            value: Some("2026071é".into()),
        };
        assert!(parse_datetime(&prop).is_none());
    }

    /// Serves `body` at /feed.ics on an ephemeral port; returns the URL.
    async fn stub_feed(body: Vec<u8>) -> String {
        let app = axum::Router::new().route(
            "/feed.ics",
            axum::routing::get(move || {
                let body = body.clone();
                async move { body }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding stub feed");
        let addr = listener.local_addr().expect("stub feed addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serving stub feed");
        });
        format!("http://{addr}/feed.ics")
    }

    #[tokio::test]
    async fn fetch_rejects_oversized_feeds_while_streaming() {
        let url = stub_feed(vec![b' '; MAX_BODY_BYTES + 1]).await;
        let err = FeedCache::default()
            .fetch(&url)
            .await
            .expect_err("oversized feed must fail");
        assert!(err.contains("exceeds"), "unexpected error: {err}");
    }
}
