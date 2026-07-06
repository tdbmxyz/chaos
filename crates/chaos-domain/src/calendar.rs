//! Calendars and events, per user.
//!
//! Two calendar kinds: `local` calendars live in the chaos database and are
//! writable; `ics` calendars subscribe to an external feed URL (Google
//! Calendar "secret address", Proton Calendar share link, any .ics) and are
//! read-only. Two-way sync (CalDAV) is a possible later addition — the
//! `kind` enum leaves room for it.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalendarKind {
    Local,
    Ics,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Calendar {
    pub id: Uuid,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub kind: CalendarKind,
    /// Feed URL for `ics` calendars (never shown truncated in the UI: it is
    /// a capability secret for Google/Proton private addresses).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ics_url: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Create/update a calendar (update = full replacement, like collections).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarRequest {
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
    pub kind: CalendarKind,
    #[serde(default)]
    pub ics_url: Option<String>,
}

/// A stored event on a local calendar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub id: Uuid,
    pub calendar_id: Uuid,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub all_day: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Create/update an event (update = full replacement).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRequest {
    pub calendar_id: Uuid,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    #[serde(default)]
    pub all_day: bool,
}

/// Query parameters of `GET /api/v1/calendar/events`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventQuery {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// One occurrence in the merged view across all of a user's calendars.
/// Local events carry their id (editable); feed events do not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEvent {
    /// Present only for events stored in chaos (editable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Uuid>,
    pub calendar_id: Uuid,
    pub calendar_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub all_day: bool,
}
