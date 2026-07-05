//! `/api/v1/calendars`, `/api/v1/calendar/events` and `/api/v1/events`:
//! per-user calendars, the merged range view (local + ICS feeds), and
//! event CRUD on local calendars. Everything requires a session.

use axum::Json;
use axum::extract::{Path, Query, State};
use chaos_domain::{
    Calendar, CalendarEvent, CalendarKind, CalendarRequest, Event, EventQuery, EventRequest,
};
use uuid::Uuid;

use crate::api::ApiError;
use crate::auth::AuthUser;
use crate::state::AppState;

pub async fn list(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<Calendar>>, ApiError> {
    Ok(Json(state.db.list_calendars(user.id).await?))
}

pub async fn create(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Json(req): Json<CalendarRequest>,
) -> Result<Json<Calendar>, ApiError> {
    Ok(Json(state.db.create_calendar(user.id, &req).await?))
}

pub async fn update(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<CalendarRequest>,
) -> Result<Json<Calendar>, ApiError> {
    let calendar = state.db.update_calendar(user.id, id, &req).await?;
    state.ics.invalidate(id).await;
    Ok(Json(calendar))
}

pub async fn delete(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.db.delete_calendar(user.id, id).await?;
    state.ics.invalidate(id).await;
    Ok(Json(serde_json::json!({})))
}

/// The merged month/range view. A broken feed only logs a warning: the
/// user's own events must never disappear because Google is slow.
pub async fn events(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Query(query): Query<EventQuery>,
) -> Result<Json<Vec<CalendarEvent>>, ApiError> {
    if query.end <= query.start {
        return Err(ApiError::Unprocessable("end must be after start".into()));
    }

    let mut out: Vec<CalendarEvent> = state
        .db
        .events_between(user.id, query.start, query.end)
        .await?
        .into_iter()
        .map(|(event, calendar_name, color)| CalendarEvent {
            id: Some(event.id),
            calendar_id: event.calendar_id,
            calendar_name,
            color,
            title: event.title,
            location: event.location,
            starts_at: event.starts_at,
            ends_at: event.ends_at,
            all_day: event.all_day,
        })
        .collect();

    for calendar in state.db.list_calendars(user.id).await? {
        if calendar.kind != CalendarKind::Ics {
            continue;
        }
        let Some(url) = &calendar.ics_url else {
            continue;
        };
        match state
            .ics
            .events(calendar.id, url, query.start, query.end)
            .await
        {
            Ok(feed_events) => out.extend(feed_events.into_iter().map(|event| CalendarEvent {
                id: None,
                calendar_id: calendar.id,
                calendar_name: calendar.name.clone(),
                color: calendar.color.clone(),
                title: event.title,
                location: event.location,
                starts_at: event.starts_at,
                ends_at: event.ends_at,
                all_day: event.all_day,
            })),
            Err(reason) => {
                tracing::warn!(calendar = calendar.name, reason, "ics feed unavailable");
            }
        }
    }

    out.sort_by_key(|event| event.starts_at);
    Ok(Json(out))
}

pub async fn create_event(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Json(req): Json<EventRequest>,
) -> Result<Json<Event>, ApiError> {
    Ok(Json(state.db.create_event(user.id, &req).await?))
}

pub async fn update_event(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<EventRequest>,
) -> Result<Json<Event>, ApiError> {
    Ok(Json(state.db.update_event(user.id, id, &req).await?))
}

pub async fn delete_event(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.db.delete_event(user.id, id).await?;
    Ok(Json(serde_json::json!({})))
}
