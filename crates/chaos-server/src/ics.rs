//! ICS feed subscriptions: fetch, parse and cache external calendars
//! (Google Calendar secret address, Proton Calendar share link, any .ics).
//!
//! The parsed feed is cached per calendar; recurrence expansion happens per
//! query because it depends on the requested range. Feeds are read-only by
//! design — writing goes to local calendars (CalDAV two-way sync would be
//! its own project).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Days, NaiveDate, NaiveDateTime, TimeZone, Utc};
use tokio::sync::RwLock;
use uuid::Uuid;

const TTL: Duration = Duration::from_secs(600);
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
/// Cap on expanded occurrences per event and range, against pathological
/// rules; a month view never legitimately needs more.
const MAX_OCCURRENCES: u16 = 400;

/// One event as parsed from a feed, before recurrence expansion.
#[derive(Debug, Clone)]
struct RawEvent {
    title: String,
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
    pub location: Option<String>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub all_day: bool,
}

/// Cached feed: when it was fetched and the parsed events.
type CachedFeed = (Instant, Arc<Vec<RawEvent>>);

pub struct FeedCache {
    entries: RwLock<HashMap<Uuid, CachedFeed>>,
    http: reqwest::Client,
}

impl Default for FeedCache {
    fn default() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
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
        if let Some((fetched, events)) = self.entries.read().await.get(&calendar_id)
            && fetched.elapsed() < TTL
        {
            return Ok(events.clone());
        }

        match self.fetch(url).await {
            Ok(events) => {
                let events = Arc::new(events);
                self.entries
                    .write()
                    .await
                    .insert(calendar_id, (Instant::now(), events.clone()));
                Ok(events)
            }
            Err(reason) => {
                // Serve the stale copy if we have one.
                if let Some((_, events)) = self.entries.read().await.get(&calendar_id) {
                    tracing::warn!(%calendar_id, reason, "ics refresh failed, serving stale feed");
                    return Ok(events.clone());
                }
                Err(reason)
            }
        }
    }

    async fn fetch(&self, url: &str) -> Result<Vec<RawEvent>, String> {
        let resp = self.http.get(url).send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("feed returned {}", resp.status()));
        }
        let body = resp.bytes().await.map_err(|e| e.to_string())?;
        if body.len() > MAX_BODY_BYTES {
            return Err(format!("feed too large ({} bytes)", body.len()));
        }
        parse(&body)
    }

    /// Drop a cached feed (after its calendar is edited or deleted).
    pub async fn invalidate(&self, calendar_id: Uuid) {
        self.entries.write().await.remove(&calendar_id);
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
        let date = NaiveDate::parse_from_str(&raw[..8.min(raw.len())], "%Y%m%d").ok()?;
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
BEGIN:VEVENT\r\nSUMMARY:Standup\r\nLOCATION:Meet\r\nDTSTART;TZID=Europe/Paris:20260701T093000\r\nDTEND;TZID=Europe/Paris:20260701T094500\r\nRRULE:FREQ=WEEKLY;BYDAY=WE\r\nEND:VEVENT\r\n\
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
    }
}
