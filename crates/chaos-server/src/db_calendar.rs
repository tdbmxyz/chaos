//! Calendar and event persistence. Every query is scoped by user id — a
//! calendar or event belonging to another user behaves exactly like one
//! that does not exist.

use chaos_domain::{Calendar, CalendarKind, CalendarRequest, Event, EventRequest};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::db::{Db, DbError, MAX_NAME_LEN, MAX_TEXT_LEN, Result, trimmed, validate_len};
use crate::db_auth::parse_uuid;

impl Db {
    // ---- calendars ----

    pub async fn list_calendars(&self, user_id: Uuid) -> Result<Vec<Calendar>> {
        let rows = sqlx::query_as::<_, CalendarRow>(
            "SELECT * FROM calendars WHERE user_id = ? ORDER BY name COLLATE NOCASE",
        )
        .bind(user_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(CalendarRow::try_into).collect()
    }

    pub async fn get_calendar(&self, user_id: Uuid, id: Uuid) -> Result<Calendar> {
        let row = sqlx::query_as::<_, CalendarRow>(
            "SELECT * FROM calendars WHERE id = ? AND user_id = ?",
        )
        .bind(id.to_string())
        .bind(user_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(DbError::NotFound)?;
        row.try_into()
    }

    pub async fn create_calendar(&self, user_id: Uuid, req: &CalendarRequest) -> Result<Calendar> {
        validate_calendar(req)?;
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO calendars (id, user_id, name, color, kind, ics_url, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(user_id.to_string())
        .bind(req.name.trim())
        .bind(&req.color)
        .bind(kind_str(req.kind))
        .bind(&req.ics_url)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        self.get_calendar(user_id, id).await
    }

    pub async fn update_calendar(
        &self,
        user_id: Uuid,
        id: Uuid,
        req: &CalendarRequest,
    ) -> Result<Calendar> {
        validate_calendar(req)?;
        let result = sqlx::query(
            "UPDATE calendars SET name = ?, color = ?, kind = ?, ics_url = ?
             WHERE id = ? AND user_id = ?",
        )
        .bind(req.name.trim())
        .bind(&req.color)
        .bind(kind_str(req.kind))
        .bind(&req.ics_url)
        .bind(id.to_string())
        .bind(user_id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        self.get_calendar(user_id, id).await
    }

    pub async fn delete_calendar(&self, user_id: Uuid, id: Uuid) -> Result<()> {
        let result = sqlx::query("DELETE FROM calendars WHERE id = ? AND user_id = ?")
            .bind(id.to_string())
            .bind(user_id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    // ---- events ----

    /// Stored events of the user's local calendars overlapping [start, end),
    /// with the calendar name/color needed by the merged view.
    pub async fn events_between(
        &self,
        user_id: Uuid,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<(Event, String, Option<String>)>> {
        let rows = sqlx::query_as::<_, EventJoinRow>(
            "SELECT e.*, c.name AS calendar_name, c.color AS calendar_color
             FROM events e JOIN calendars c ON c.id = e.calendar_id
             WHERE c.user_id = ? AND e.starts_at < ? AND e.ends_at > ?
             ORDER BY e.starts_at",
        )
        .bind(user_id.to_string())
        .bind(end)
        .bind(start)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let name = row.calendar_name.clone();
                let color = row.calendar_color.clone();
                Ok((row.event.try_into()?, name, color))
            })
            .collect()
    }

    pub async fn create_event(&self, user_id: Uuid, req: &EventRequest) -> Result<Event> {
        self.ensure_writable_calendar(user_id, req.calendar_id)
            .await?;
        validate_event(req)?;
        let id = Uuid::now_v7();
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO events (id, calendar_id, title, description, location,
                                 starts_at, ends_at, all_day, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(req.calendar_id.to_string())
        .bind(req.title.trim())
        .bind(trimmed(&req.description))
        .bind(trimmed(&req.location))
        .bind(req.starts_at)
        .bind(req.ends_at)
        .bind(req.all_day)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.get_event(user_id, id).await
    }

    pub async fn get_event(&self, user_id: Uuid, id: Uuid) -> Result<Event> {
        let row = sqlx::query_as::<_, EventRow>(
            "SELECT e.* FROM events e JOIN calendars c ON c.id = e.calendar_id
             WHERE e.id = ? AND c.user_id = ?",
        )
        .bind(id.to_string())
        .bind(user_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(DbError::NotFound)?;
        row.try_into()
    }

    pub async fn update_event(&self, user_id: Uuid, id: Uuid, req: &EventRequest) -> Result<Event> {
        // The event must already be the user's; the (possibly new) target
        // calendar must be the user's and writable.
        self.get_event(user_id, id).await?;
        self.ensure_writable_calendar(user_id, req.calendar_id)
            .await?;
        validate_event(req)?;
        let result = sqlx::query(
            "UPDATE events SET calendar_id = ?, title = ?, description = ?, location = ?,
                               starts_at = ?, ends_at = ?, all_day = ?, updated_at = ?
             WHERE id = ? AND calendar_id IN
             (SELECT id FROM calendars WHERE user_id = ?)",
        )
        .bind(req.calendar_id.to_string())
        .bind(req.title.trim())
        .bind(trimmed(&req.description))
        .bind(trimmed(&req.location))
        .bind(req.starts_at)
        .bind(req.ends_at)
        .bind(req.all_day)
        .bind(Utc::now())
        .bind(id.to_string())
        .bind(user_id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        self.get_event(user_id, id).await
    }

    pub async fn delete_event(&self, user_id: Uuid, id: Uuid) -> Result<()> {
        let result = sqlx::query(
            "DELETE FROM events WHERE id = ? AND calendar_id IN
             (SELECT id FROM calendars WHERE user_id = ?)",
        )
        .bind(id.to_string())
        .bind(user_id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    async fn ensure_writable_calendar(&self, user_id: Uuid, calendar_id: Uuid) -> Result<()> {
        let calendar = self.get_calendar(user_id, calendar_id).await.map_err(|e| {
            if matches!(e, DbError::NotFound) {
                DbError::Constraint("calendar does not exist".into())
            } else {
                e
            }
        })?;
        if calendar.kind != CalendarKind::Local {
            return Err(DbError::Constraint(
                "events can only be created on local calendars (feeds are read-only)".into(),
            ));
        }
        Ok(())
    }
}

fn validate_calendar(req: &CalendarRequest) -> Result<()> {
    if req.name.trim().is_empty() {
        return Err(DbError::Constraint("name cannot be empty".into()));
    }
    validate_len("name", &req.name, MAX_NAME_LEN)?;
    if let Some(url) = &req.ics_url {
        validate_len("ics_url", url, MAX_TEXT_LEN)?;
    }
    if req.kind == CalendarKind::Ics && req.ics_url.as_deref().unwrap_or("").trim().is_empty() {
        return Err(DbError::Constraint("feed calendars need an ics_url".into()));
    }
    Ok(())
}

fn validate_event(req: &EventRequest) -> Result<()> {
    if req.title.trim().is_empty() {
        return Err(DbError::Constraint("title cannot be empty".into()));
    }
    validate_len("title", &req.title, MAX_NAME_LEN)?;
    if let Some(description) = &req.description {
        validate_len("description", description, MAX_TEXT_LEN)?;
    }
    if let Some(location) = &req.location {
        validate_len("location", location, MAX_NAME_LEN)?;
    }
    if req.ends_at <= req.starts_at {
        return Err(DbError::Constraint("event must end after it starts".into()));
    }
    Ok(())
}

fn kind_str(kind: CalendarKind) -> &'static str {
    match kind {
        CalendarKind::Local => "local",
        CalendarKind::Ics => "ics",
    }
}

#[derive(sqlx::FromRow)]
struct CalendarRow {
    id: String,
    #[allow(dead_code)]
    user_id: String,
    name: String,
    color: Option<String>,
    kind: String,
    ics_url: Option<String>,
    created_at: DateTime<Utc>,
}

impl TryFrom<CalendarRow> for Calendar {
    type Error = DbError;

    fn try_from(row: CalendarRow) -> Result<Calendar> {
        let kind = match row.kind.as_str() {
            "local" => CalendarKind::Local,
            "ics" => CalendarKind::Ics,
            other => return Err(DbError::Corrupt(format!("calendar kind {other:?}"))),
        };
        Ok(Calendar {
            id: parse_uuid(&row.id)?,
            name: row.name,
            color: row.color,
            kind,
            ics_url: row.ics_url,
            created_at: row.created_at,
        })
    }
}

#[derive(sqlx::FromRow)]
struct EventRow {
    id: String,
    calendar_id: String,
    title: String,
    description: Option<String>,
    location: Option<String>,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    all_day: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct EventJoinRow {
    #[sqlx(flatten)]
    event: EventRow,
    calendar_name: String,
    calendar_color: Option<String>,
}

impl TryFrom<EventRow> for Event {
    type Error = DbError;

    fn try_from(row: EventRow) -> Result<Event> {
        Ok(Event {
            id: parse_uuid(&row.id)?,
            calendar_id: parse_uuid(&row.calendar_id)?,
            title: row.title,
            description: row.description,
            location: row.location,
            starts_at: row.starts_at,
            ends_at: row.ends_at,
            all_day: row.all_day,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 10, h, 0, 0).unwrap()
    }

    async fn setup() -> (Db, Uuid, Calendar) {
        let db = Db::in_memory().await.expect("db");
        let user = db.create_user("tibo", "Tibo", "x").await.expect("user");
        let calendar = db
            .create_calendar(
                user.id,
                &CalendarRequest {
                    name: "Perso".into(),
                    color: Some("#7c9aff".into()),
                    kind: CalendarKind::Local,
                    ics_url: None,
                },
            )
            .await
            .expect("calendar");
        (db, user.id, calendar)
    }

    #[tokio::test]
    async fn event_crud_scoped_by_user() {
        let (db, user_id, calendar) = setup().await;
        let other = db.create_user("so", "SO", "x").await.expect("other user");

        let event = db
            .create_event(
                user_id,
                &EventRequest {
                    calendar_id: calendar.id,
                    title: "Dentist".into(),
                    description: None,
                    location: Some("Lyon".into()),
                    starts_at: ts(9),
                    ends_at: ts(10),
                    all_day: false,
                },
            )
            .await
            .expect("event");

        // Range query finds it with calendar metadata attached.
        let found = db
            .events_between(user_id, ts(0), ts(23))
            .await
            .expect("range");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1, "Perso");

        // Another user sees nothing and cannot touch it.
        assert!(
            db.events_between(other.id, ts(0), ts(23))
                .await
                .expect("other range")
                .is_empty()
        );
        assert!(matches!(
            db.delete_event(other.id, event.id).await,
            Err(DbError::NotFound)
        ));

        // Another user cannot update it either. (This exercises the
        // get_event pre-check; the UPDATE's own owner scoping is defense
        // in depth behind it and isn't separately reachable from here.)
        assert!(matches!(
            db.update_event(
                other.id,
                event.id,
                &EventRequest {
                    calendar_id: calendar.id,
                    title: "Hijacked".into(),
                    description: None,
                    location: None,
                    starts_at: ts(9),
                    ends_at: ts(10),
                    all_day: false,
                },
            )
            .await,
            Err(DbError::NotFound)
        ));
        assert_eq!(
            db.get_event(user_id, event.id)
                .await
                .expect("still ours")
                .title,
            "Dentist"
        );

        db.delete_event(user_id, event.id).await.expect("delete");
    }

    #[tokio::test]
    async fn feed_calendars_are_read_only() {
        let (db, user_id, _) = setup().await;
        let feed = db
            .create_calendar(
                user_id,
                &CalendarRequest {
                    name: "Holidays".into(),
                    color: None,
                    kind: CalendarKind::Ics,
                    ics_url: Some("https://example.com/basic.ics".into()),
                },
            )
            .await
            .expect("feed calendar");

        let err = db
            .create_event(
                user_id,
                &EventRequest {
                    calendar_id: feed.id,
                    title: "Nope".into(),
                    description: None,
                    location: None,
                    starts_at: ts(9),
                    ends_at: ts(10),
                    all_day: false,
                },
            )
            .await;
        assert!(matches!(err, Err(DbError::Constraint(_))));

        // ICS calendars without a URL are refused.
        assert!(matches!(
            db.create_calendar(
                user_id,
                &CalendarRequest {
                    name: "Broken".into(),
                    color: None,
                    kind: CalendarKind::Ics,
                    ics_url: None,
                },
            )
            .await,
            Err(DbError::Constraint(_))
        ));
    }

    #[tokio::test]
    async fn oversized_calendar_and_event_fields_are_refused() {
        let (db, user_id, calendar) = setup().await;

        assert!(matches!(
            db.create_calendar(
                user_id,
                &CalendarRequest {
                    name: "Feeds".into(),
                    color: None,
                    kind: CalendarKind::Ics,
                    ics_url: Some(format!("https://example.com/{}", "x".repeat(5000))),
                },
            )
            .await,
            Err(DbError::Constraint(_))
        ));

        assert!(matches!(
            db.create_event(
                user_id,
                &EventRequest {
                    calendar_id: calendar.id,
                    title: "x".repeat(513),
                    description: None,
                    location: None,
                    starts_at: ts(9),
                    ends_at: ts(10),
                    all_day: false,
                },
            )
            .await,
            Err(DbError::Constraint(_))
        ));

        assert!(matches!(
            db.create_event(
                user_id,
                &EventRequest {
                    calendar_id: calendar.id,
                    title: "Fine".into(),
                    description: Some("x".repeat(5000)),
                    location: None,
                    starts_at: ts(9),
                    ends_at: ts(10),
                    all_day: false,
                },
            )
            .await,
            Err(DbError::Constraint(_))
        ));
    }
}
