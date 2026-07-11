//! Push notifications via [ntfy](https://ntfy.sh): service down/recovered
//! alerts from the monitor and calendar event reminders.
//!
//! Everything here is best-effort by design — a failed notification is a
//! warning in the log, never an error that reaches a caller. The feature
//! is fully off (no client, no task) when `[notifications].ntfy_url` is
//! unset.

use std::collections::HashSet;
use std::time::Duration;

use chaos_domain::{CalendarKind, HealthState};
use chrono::{DateTime, Utc};
use url::Url;
use uuid::Uuid;

use crate::config::NotificationsConfig;
use crate::state::AppState;

const TIMEOUT: Duration = Duration::from_secs(10);

pub struct Notifier {
    http: reqwest::Client,
    /// `{ntfy_url}/{topic}` — ntfy publishes with a plain POST to the topic.
    endpoint: Url,
    token: Option<String>,
}

impl Notifier {
    /// `None` when notifications aren't configured (`ntfy_url` unset).
    pub fn new(config: &NotificationsConfig) -> anyhow::Result<Option<Self>> {
        let Some(base) = config.ntfy_url.clone() else {
            return Ok(None);
        };
        let topic = config.topic.trim();
        anyhow::ensure!(
            !topic.is_empty(),
            "notifications.ntfy_url is set but notifications.topic is empty"
        );
        let endpoint = base
            .join(topic)
            .map_err(|e| anyhow::anyhow!("joining ntfy topic onto {base}: {e}"))?;
        Ok(Some(Self {
            http: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .user_agent(concat!("chaos/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("building ntfy http client"),
            endpoint,
            token: config.token.clone(),
        }))
    }

    /// Publish one notification. Best-effort: failures are logged and
    /// swallowed — a dead ntfy server must never take down chaos.
    pub async fn send(&self, title: &str, message: &str, tags: &str, priority: &str) {
        let mut request = self
            .http
            .post(self.endpoint.clone())
            .header("Title", title)
            .header("Tags", tags)
            .header("Priority", priority)
            .body(message.to_string());
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        match request.send().await {
            Ok(resp) if !resp.status().is_success() => {
                tracing::warn!(status = %resp.status(), title, "ntfy rejected notification");
            }
            Ok(_) => tracing::debug!(title, "notification sent"),
            Err(err) => tracing::warn!(error = %err, title, "ntfy send failed"),
        }
    }
}

/// Sweeps a state must survive before it can notify — debounces flapping
/// services (one bad probe in isolation is noise, two in a row is news).
const ALERT_AFTER_CHECKS: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceAlert {
    Down,
    Recovered,
}

/// Per-service transition detector: pure, driven by the monitor once per
/// sweep. `Down`/`Degraded` are alert-worthy; `Up` is healthy; `Paused`,
/// `Starting` and `Unknown` are neutral — they break streaks but never
/// notify (a deliberately stopped on-demand unit is not an outage).
#[derive(Debug, Default)]
pub struct AlertTracker {
    /// Whether the last alert we sent said "down". Starts false, so a
    /// service that is up from boot stays silent, while one that is down
    /// from boot alerts once debounced.
    notified_down: bool,
    /// Direction of the current streak of consecutive identical readings.
    streak_down: bool,
    streak: u8,
}

impl AlertTracker {
    pub fn observe(&mut self, state: HealthState) -> Option<ServiceAlert> {
        let down = match state {
            HealthState::Down | HealthState::Degraded => true,
            HealthState::Up => false,
            HealthState::Paused | HealthState::Starting | HealthState::Unknown => {
                self.streak = 0;
                return None;
            }
        };
        if self.streak > 0 && down == self.streak_down {
            self.streak = self.streak.saturating_add(1);
        } else {
            self.streak_down = down;
            self.streak = 1;
        }
        if self.streak < ALERT_AFTER_CHECKS || down == self.notified_down {
            return None;
        }
        self.notified_down = down;
        Some(if down {
            ServiceAlert::Down
        } else {
            ServiceAlert::Recovered
        })
    }
}

/// True when the event starts within `[now, now + lead)`. Range queries
/// return events *overlapping* a window (an ongoing meeting included);
/// reminders only care about ones that *start* in it.
pub fn due(starts_at: DateTime<Utc>, now: DateTime<Utc>, lead: chrono::Duration) -> bool {
    starts_at >= now && starts_at < now + lead
}

/// Identity of one notified occurrence. `calendar_id + starts_at + title`
/// covers local events and feed occurrences alike (feed events have no id)
/// and distinguishes RRULE occurrences by their start.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReminderKey {
    pub calendar_id: Uuid,
    pub starts_at: DateTime<Utc>,
    pub title: String,
}

/// In-memory dedup of sent reminders — deliberately not persisted: a
/// server restart re-notifying an event still 10 minutes out is harmless,
/// a DB migration for it is not worth it.
#[derive(Debug, Default)]
pub struct ReminderLog {
    sent: HashSet<ReminderKey>,
}

impl ReminderLog {
    /// True exactly once per key.
    pub fn first_time(&mut self, key: ReminderKey) -> bool {
        self.sent.insert(key)
    }

    /// Forget occurrences that started more than two hours ago so the set
    /// cannot grow forever.
    pub fn prune(&mut self, now: DateTime<Utc>) {
        self.sent
            .retain(|key| key.starts_at > now - chrono::Duration::hours(2));
    }
}

const SCAN_INTERVAL: Duration = Duration::from_secs(60);

/// Spawn the calendar reminder scanner. Callers gate on configuration
/// (notifier present + `calendar_reminders` on); the guard here is a
/// safety net only.
pub fn spawn_reminders(state: AppState) {
    tokio::spawn(run_reminders(state));
}

async fn run_reminders(state: AppState) {
    let Some(notifier) = state.notifier.clone() else {
        return;
    };
    let lead = chrono::Duration::minutes(state.config.notifications.reminder_lead_minutes as i64);
    let mut log = ReminderLog::default();

    loop {
        let now = Utc::now();
        if let Err(reason) = scan(&state, &notifier, &mut log, now, lead).await {
            tracing::warn!(reason, "calendar reminder scan failed");
        }
        log.prune(now);
        tokio::time::sleep(SCAN_INTERVAL).await;
    }
}

/// One pass: every user's local events and ICS feeds, window `[now,
/// now+lead)`. All-day events are skipped (a lead-minutes ping around
/// midnight UTC is noise, not a reminder).
async fn scan(
    state: &AppState,
    notifier: &Notifier,
    log: &mut ReminderLog,
    now: DateTime<Utc>,
    lead: chrono::Duration,
) -> Result<(), String> {
    let horizon = now + lead;
    let users = state.db.list_users().await.map_err(|e| e.to_string())?;
    for user in users {
        let events = state
            .db
            .events_between(user.id, now, horizon)
            .await
            .map_err(|e| e.to_string())?;
        for (event, _calendar_name, _color) in events {
            let key = ReminderKey {
                calendar_id: event.calendar_id,
                starts_at: event.starts_at,
                title: event.title.clone(),
            };
            if !event.all_day && due(event.starts_at, now, lead) && log.first_time(key) {
                send_reminder(
                    notifier,
                    &event.title,
                    event.starts_at,
                    event.location.as_deref(),
                    now,
                )
                .await;
            }
        }

        let calendars = state
            .db
            .list_calendars(user.id)
            .await
            .map_err(|e| e.to_string())?;
        for calendar in calendars {
            if calendar.kind != CalendarKind::Ics {
                continue;
            }
            let Some(url) = &calendar.ics_url else {
                continue;
            };
            match state.ics.events(calendar.id, url, now, horizon).await {
                Ok(feed_events) => {
                    for event in feed_events {
                        let key = ReminderKey {
                            calendar_id: calendar.id,
                            starts_at: event.starts_at,
                            title: event.title.clone(),
                        };
                        if !event.all_day && due(event.starts_at, now, lead) && log.first_time(key)
                        {
                            send_reminder(
                                notifier,
                                &event.title,
                                event.starts_at,
                                event.location.as_deref(),
                                now,
                            )
                            .await;
                        }
                    }
                }
                Err(reason) => {
                    tracing::warn!(
                        calendar = calendar.name,
                        reason,
                        "ics feed unavailable for reminders"
                    );
                }
            }
        }
    }
    Ok(())
}

/// Relative time on purpose: the server does not know the user's timezone,
/// and "in 12 min" is what a reminder is anyway.
async fn send_reminder(
    notifier: &Notifier,
    title: &str,
    starts_at: DateTime<Utc>,
    location: Option<&str>,
    now: DateTime<Utc>,
) {
    let minutes = (starts_at - now).num_minutes().max(0);
    let message = match location {
        Some(location) => format!("starts in {minutes} min — {location}"),
        None => format!("starts in {minutes} min"),
    };
    notifier.send(title, &message, "calendar", "default").await;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::http::HeaderMap;
    use chrono::{Duration as ChronoDuration, TimeZone, Utc};
    use tokio::sync::Mutex;
    use url::Url;
    use uuid::Uuid;

    use super::*;
    use crate::config::NotificationsConfig;

    /// One request as seen by the stub ntfy server.
    #[derive(Debug, Clone)]
    struct Captured {
        path: String,
        title: Option<String>,
        priority: Option<String>,
        tags: Option<String>,
        authorization: Option<String>,
        body: String,
    }

    /// Stub ntfy: captures every POST and answers 200.
    async fn stub_ntfy() -> (Url, Arc<Mutex<Vec<Captured>>>) {
        let captured: Arc<Mutex<Vec<Captured>>> = Arc::default();
        let sink = captured.clone();
        let app = axum::Router::new().fallback(axum::routing::post(
            move |uri: axum::http::Uri, headers: HeaderMap, body: String| {
                let sink = sink.clone();
                let header = |name: &str| {
                    headers
                        .get(name)
                        .and_then(|v| v.to_str().ok())
                        .map(String::from)
                };
                let entry = Captured {
                    path: uri.path().to_string(),
                    title: header("title"),
                    priority: header("priority"),
                    tags: header("tags"),
                    authorization: header("authorization"),
                    body,
                };
                async move {
                    sink.lock().await.push(entry);
                    "ok"
                }
            },
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding stub ntfy");
        let addr = listener.local_addr().expect("stub ntfy addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serving stub ntfy");
        });
        (
            format!("http://{addr}/").parse().expect("stub ntfy url"),
            captured,
        )
    }

    fn config(url: Url, token: Option<&str>) -> NotificationsConfig {
        NotificationsConfig {
            ntfy_url: Some(url),
            topic: "chaos".into(),
            token: token.map(String::from),
            ..NotificationsConfig::default()
        }
    }

    #[tokio::test]
    async fn send_posts_to_the_topic_with_headers_and_bearer() {
        let (url, captured) = stub_ntfy().await;
        let notifier = Notifier::new(&config(url, Some("tk_secret")))
            .expect("building notifier")
            .expect("notifier is configured");

        notifier
            .send(
                "jellyfin is down",
                "connection refused",
                "rotating_light",
                "high",
            )
            .await;

        let requests = captured.lock().await;
        assert_eq!(requests.len(), 1);
        let req = &requests[0];
        assert_eq!(req.path, "/chaos");
        assert_eq!(req.title.as_deref(), Some("jellyfin is down"));
        assert_eq!(req.tags.as_deref(), Some("rotating_light"));
        assert_eq!(req.priority.as_deref(), Some("high"));
        assert_eq!(req.authorization.as_deref(), Some("Bearer tk_secret"));
        assert_eq!(req.body, "connection refused");
    }

    #[tokio::test]
    async fn send_survives_an_unreachable_server() {
        // Nothing listens on this port; send must log and return, not panic.
        let notifier = Notifier::new(&config("http://127.0.0.1:1/".parse().expect("url"), None))
            .expect("building notifier")
            .expect("notifier is configured");
        notifier.send("t", "m", "calendar", "default").await;
    }

    #[test]
    fn unconfigured_and_misconfigured_notifier() {
        // No ntfy_url → feature off.
        assert!(
            Notifier::new(&NotificationsConfig::default())
                .expect("no url is fine")
                .is_none()
        );
        // URL without a topic is a startup error, not a silent no-op.
        let broken = NotificationsConfig {
            ntfy_url: Some("https://ntfy.sh".parse().expect("url")),
            ..NotificationsConfig::default()
        };
        assert!(Notifier::new(&broken).is_err());
    }

    use chaos_domain::HealthState;

    #[test]
    fn alerts_only_after_two_consecutive_checks() {
        use HealthState::*;
        let mut t = AlertTracker::default();
        assert_eq!(t.observe(Up), None);
        assert_eq!(t.observe(Down), None, "first down is not yet an alert");
        assert_eq!(t.observe(Down), Some(ServiceAlert::Down));
        assert_eq!(t.observe(Down), None, "already notified");
        assert_eq!(t.observe(Up), None, "first up is not yet a recovery");
        assert_eq!(t.observe(Up), Some(ServiceAlert::Recovered));
        assert_eq!(t.observe(Up), None);
    }

    #[test]
    fn flapping_never_alerts() {
        use HealthState::*;
        let mut t = AlertTracker::default();
        for state in [Up, Down, Up, Down, Up, Down, Up] {
            assert_eq!(t.observe(state), None, "flap on {state:?} must stay silent");
        }
    }

    #[test]
    fn degraded_counts_as_down_and_neutral_states_break_streaks() {
        use HealthState::*;
        let mut t = AlertTracker::default();
        assert_eq!(t.observe(Degraded), None);
        assert_eq!(
            t.observe(Degraded),
            Some(ServiceAlert::Down),
            "5xx alerts too"
        );
        // Paused/Starting/Unknown never alert and break a forming streak.
        assert_eq!(t.observe(Up), None);
        assert_eq!(t.observe(Paused), None);
        assert_eq!(t.observe(Up), None, "streak restarted by the neutral state");
        assert_eq!(t.observe(Up), Some(ServiceAlert::Recovered));
    }

    #[test]
    fn service_down_from_boot_still_alerts() {
        use HealthState::*;
        let mut t = AlertTracker::default();
        assert_eq!(t.observe(Down), None);
        assert_eq!(t.observe(Down), Some(ServiceAlert::Down));
    }

    fn at(h: u32, m: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 11, h, m, 0).unwrap()
    }

    #[test]
    fn due_matches_events_starting_within_the_lead_window() {
        let now = at(9, 0);
        let lead = ChronoDuration::minutes(15);
        assert!(due(at(9, 0), now, lead), "starting right now is due");
        assert!(due(at(9, 14), now, lead));
        assert!(!due(at(9, 15), now, lead), "window end is exclusive");
        assert!(!due(at(8, 59), now, lead), "already started is not due");
    }

    #[test]
    fn reminder_log_dedupes_and_prunes() {
        let mut log = ReminderLog::default();
        let key = ReminderKey {
            calendar_id: Uuid::nil(),
            starts_at: at(9, 0),
            title: "Dentist".into(),
        };
        assert!(log.first_time(key.clone()));
        assert!(!log.first_time(key.clone()), "second scan must not re-send");

        // Another occurrence of the same recurring event is a new key.
        let next = ReminderKey {
            starts_at: at(10, 0),
            ..key.clone()
        };
        assert!(log.first_time(next));

        // Pruning forgets long-past starts; recent ones survive.
        log.prune(at(11, 30));
        assert!(log.first_time(key), "9:00 pruned at 11:30 (>2h past)");
        let recent = ReminderKey {
            calendar_id: Uuid::nil(),
            starts_at: at(10, 0),
            title: "Dentist".into(),
        };
        assert!(!log.first_time(recent), "10:00 still remembered at 11:30");
    }
}
