# Ntfy Notifications Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Server-side push notifications via ntfy (roadmap Phase 8): alert when a monitored service goes down / recovers (with flap debouncing), and remind about calendar events starting soon (local events + ICS feeds, all users). No web push. No config section → feature fully off, zero behavior change.

**Architecture:** One new module `crates/chaos-server/src/notify.rs` holding a `Notifier` (HTTP POST to `{ntfy_url}/{topic}` with `Title`/`Tags`/`Priority` headers, optional bearer token), a pure `AlertTracker` state machine for monitor transitions (debounce: alert only after 2 consecutive checks in the new state), and a pure reminder window/dedup layer (`due()` + `ReminderLog`, in-memory `HashSet<ReminderKey>` with pruning — no DB migration). The monitor task observes each sweep's results through per-service trackers and sends alerts after releasing the `statuses` write lock. A new reminder task (spawned in `main.rs` like `monitor::spawn`/`archiver::spawn`, only when configured) scans every minute: `db.list_users()` → `db.events_between(user, now, now+lead)` for local events, `FeedCache::events(calendar_id, url, now, now+lead)` for ICS calendars (both already exist and need no authenticated request — they take a `user_id`/`calendar_id` directly). Sending never propagates errors: log-and-continue, 10 s client timeout.

**Tech Stack:** Rust, axum (stub servers in tests only), reqwest, figment + serde (config), chrono, tokio. Tests run with `cargo nextest run` (workspace convention, see `justfile`). Test HTTP stubbing follows the existing `stub_ha` pattern in `crates/chaos-server/src/home_assistant.rs`.

---

## Design decisions locked in

- **States** (`chaos_domain::HealthState`): `Up` = good; `Down` and `Degraded` (5xx) = alert-worthy; `Paused` (deliberately stopped on-demand unit), `Starting`, `Unknown` = neutral — they break any streak and never alert.
- **Debounce:** a state must be observed on 2 consecutive sweeps before it can notify, and we only notify on a change relative to the last *notified* state (`notified_down` flag). A service that is down at startup does alert (it *is* down); a service that is up at startup does not.
- **Reminder window:** event *starts* within `[now, now + lead)`. `events_between` returns events *overlapping* the range, so the pure `due()` filter narrows to actual starts. All-day events are skipped (they start at 00:00 UTC; a lead-minutes ping at 23:45 the previous day is noise).
- **Dedup key:** `(calendar_id, starts_at, title)` — works for both local events (which have ids) and feed occurrences (which don't). RRULE occurrences differ in `starts_at`, so each occurrence notifies once. Pruned when `starts_at` is more than 2 h in the past.
- **Reminder message** uses relative time ("starts in 12 min"), never wall-clock — the server does not know the user's timezone and ntfy shows delivery time anyway.
- **One topic for the household:** if two users subscribe to the same feed, each subscription is its own calendar row (own `calendar_id`), so the topic may receive one ping per subscribing user. Accepted — this is a household dashboard with a shared topic.
- **Secrets:** `token` is a plain config value (like the rest of `chaos.toml`; on NixOS the generated config already lives root-readable in the store — same posture as today, and ntfy tokens are low-value LAN credentials).

---

## Task 1: `[notifications]` config section

**Files:**
- `crates/chaos-server/src/config.rs`

### Steps

- [ ] Add a failing test to the existing `tests` module at the bottom of `crates/chaos-server/src/config.rs`:

```rust
    #[test]
    fn notifications_section_parses_and_defaults_off() {
        let toml = r#"
            [notifications]
            ntfy_url = "https://ntfy.example.com"
            topic = "chaos"
            token = "tk_secret"
            reminder_lead_minutes = 30
        "#;
        let config: super::Config = figment::Figment::from(
            figment::providers::Serialized::defaults(super::Config::default()),
        )
        .merge(figment::providers::Toml::string(toml))
        .extract()
        .expect("notifications section must parse");
        let n = &config.notifications;
        assert_eq!(
            n.ntfy_url.as_ref().map(url::Url::as_str),
            Some("https://ntfy.example.com/")
        );
        assert_eq!(n.topic, "chaos");
        assert_eq!(n.token.as_deref(), Some("tk_secret"));
        assert!(n.service_alerts, "service_alerts defaults to true");
        assert!(n.calendar_reminders, "calendar_reminders defaults to true");
        assert_eq!(n.reminder_lead_minutes, 30);

        // No section at all → feature off (no url), defaults intact.
        let default = super::Config::default();
        assert!(default.notifications.ntfy_url.is_none());
        assert_eq!(default.notifications.reminder_lead_minutes, 15);
    }
```

- [ ] Run `cargo nextest run -p chaos-server notifications_section` — expect a **compile error** (`no field notifications on Config`). That is the red state.
- [ ] Implement. In `crates/chaos-server/src/config.rs`, add the struct after `HomeAssistantConfig`:

```rust
/// Push notifications via ntfy. The whole feature is off when `ntfy_url`
/// is `None` (the default): no HTTP client, no background task, nothing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationsConfig {
    /// ntfy server base URL, e.g. `https://ntfy.sh` or a self-hosted
    /// instance. `None` disables notifications entirely.
    pub ntfy_url: Option<url::Url>,
    /// Topic notifications are published to (`{ntfy_url}/{topic}`).
    pub topic: String,
    /// Bearer token for protected topics (ntfy access tokens).
    pub token: Option<String>,
    /// Notify on service Down/Degraded ↔ Up transitions.
    pub service_alerts: bool,
    /// Remind about calendar events shortly before they start.
    pub calendar_reminders: bool,
    /// How long before an event starts the reminder fires.
    pub reminder_lead_minutes: u64,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            ntfy_url: None,
            topic: String::new(),
            token: None,
            service_alerts: true,
            calendar_reminders: true,
            reminder_lead_minutes: 15,
        }
    }
}
```

- [ ] Add the field to `Config` (after `home_assistant`):

```rust
    /// Push notifications via ntfy (service alerts + calendar reminders).
    /// Feature is off when `ntfy_url` is `None`.
    pub notifications: NotificationsConfig,
```

  and to `impl Default for Config`:

```rust
            notifications: NotificationsConfig::default(),
```

- [ ] Run `cargo nextest run -p chaos-server notifications_section` — green. Then `cargo nextest run -p chaos-server` to confirm nothing else broke.
- [ ] Run `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] Commit:

```bash
cd /projects/rust/chaos
git add crates/chaos-server/src/config.rs
git commit -m "$(cat <<'EOF'
feat(server): [notifications] config section for ntfy

ntfy_url/topic/token plus service_alerts, calendar_reminders and
reminder_lead_minutes toggles; absent section keeps the feature off.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

---

## Task 2: `Notifier` + stub ntfy server test

**Files:**
- `crates/chaos-server/src/notify.rs` (new)
- `crates/chaos-server/src/main.rs` (register module)
- `crates/chaos-server/src/state.rs` (hold `Option<Arc<Notifier>>`)

### Steps

- [ ] Create `crates/chaos-server/src/notify.rs` with the module doc, the `Notifier` skeleton is written in the next implementation step — start with the failing test file containing only the test module so the red state is a compile error against the missing types. Practical TDD in Rust: write the whole file below, but **first** write only the `#[cfg(test)] mod tests` block (plus `use` lines it needs) and run to see it fail to compile; then fill in the implementation above it. Test module (modeled on `stub_ha` in `home_assistant.rs`):

```rust
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::http::HeaderMap;
    use tokio::sync::Mutex;
    use url::Url;

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
                let header =
                    |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(String::from);
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
        (format!("http://{addr}/").parse().expect("stub ntfy url"), captured)
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
            .send("jellyfin is down", "connection refused", "rotating_light", "high")
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
        let notifier = Notifier::new(&config(
            "http://127.0.0.1:1/".parse().expect("url"),
            None,
        ))
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
}
```

- [ ] Register the module in `crates/chaos-server/src/main.rs` (alphabetical, between `mod monitor;` and `mod state;`):

```rust
mod notify;
```

- [ ] Run `cargo nextest run -p chaos-server notify` — expect a **compile error** (`cannot find Notifier`). Red.
- [ ] Implement in `crates/chaos-server/src/notify.rs` above the test module:

```rust
//! Push notifications via [ntfy](https://ntfy.sh): service down/recovered
//! alerts from the monitor and calendar event reminders.
//!
//! Everything here is best-effort by design — a failed notification is a
//! warning in the log, never an error that reaches a caller. The feature
//! is fully off (no client, no task) when `[notifications].ntfy_url` is
//! unset.

use std::time::Duration;

use url::Url;

use crate::config::NotificationsConfig;

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
```

- [ ] Wire the notifier into `AppState` (`crates/chaos-server/src/state.rs`). Add the import and field:

```rust
use crate::notify::Notifier;
```

  field on `AppState` (after `home`):

```rust
    /// ntfy publisher, when `[notifications]` is configured.
    pub notifier: Option<Arc<Notifier>>,
```

  and in `AppState::new`, mirroring the `home` line:

```rust
        let notifier = Notifier::new(&config.notifications)?.map(Arc::new);
```

  with `notifier,` added to the `Ok(Self { ... })` initializer.

- [ ] Run `cargo nextest run -p chaos-server notify` — green. Then the full `cargo nextest run -p chaos-server`.
- [ ] Run `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] Commit:

```bash
cd /projects/rust/chaos
git add crates/chaos-server/src/notify.rs crates/chaos-server/src/main.rs crates/chaos-server/src/state.rs
git commit -m "$(cat <<'EOF'
feat(server): ntfy Notifier (POST to topic, bearer, best-effort)

Built once in AppState::new when [notifications] is configured; send()
logs failures and never propagates them.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

---

## Task 3: service alerts — pure `AlertTracker` + monitor wiring

**Files:**
- `crates/chaos-server/src/notify.rs` (tracker + tests)
- `crates/chaos-server/src/monitor.rs` (wiring)

### Steps

- [ ] Add failing unit tests to the `tests` module in `crates/chaos-server/src/notify.rs` (add `use chaos_domain::HealthState;` to the test imports):

```rust
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
        assert_eq!(t.observe(Degraded), Some(ServiceAlert::Down), "5xx alerts too");
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
```

- [ ] Run `cargo nextest run -p chaos-server notify` — compile error (`AlertTracker` unknown). Red.
- [ ] Implement in `crates/chaos-server/src/notify.rs` (below `Notifier`; add `use chaos_domain::HealthState;` at the top):

```rust
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
```

  Note: if `HealthState` has variants beyond `Up/Degraded/Down/Paused/Starting/Unknown` (check `crates/chaos-domain/src/service.rs`), fold extras into the neutral arm.

- [ ] Run `cargo nextest run -p chaos-server notify` — green.
- [ ] Wire the monitor (`crates/chaos-server/src/monitor.rs`). Add imports:

```rust
use std::collections::HashMap;

use crate::notify::{AlertTracker, ServiceAlert};
```

  In `run()`, before the loop:

```rust
    let alerting = state.notifier.is_some() && state.config.notifications.service_alerts;
    let mut trackers: HashMap<String, AlertTracker> = HashMap::new();
```

  Replace the statuses-write block (collect alerts inside the lock, send after dropping it — a slow ntfy must not hold `statuses` for readers):

```rust
        let mut alerts: Vec<(String, ServiceAlert, Option<String>)> = Vec::new();
        {
            let mut statuses = state.statuses.write().await;
            for (service, status) in state.config.services.iter().zip(results) {
                if alerting
                    && let Some(alert) = trackers
                        .entry(service.id.clone())
                        .or_default()
                        .observe(status.state)
                {
                    alerts.push((service.title.clone(), alert, status.error.clone()));
                }
                statuses.insert(service.id.clone(), status);
            }
        }
        if let Some(notifier) = &state.notifier {
            for (title, alert, error) in alerts {
                match alert {
                    ServiceAlert::Down => {
                        let message = error.unwrap_or_else(|| "health check failing".into());
                        notifier
                            .send(&format!("{title} is down"), &message, "rotating_light", "high")
                            .await;
                    }
                    ServiceAlert::Recovered => {
                        notifier
                            .send(
                                &format!("{title} recovered"),
                                "health check passing again",
                                "white_check_mark",
                                "default",
                            )
                            .await;
                    }
                }
            }
        }
```

- [ ] Run `cargo nextest run -p chaos-server` — full crate green.
- [ ] Run `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] Commit:

```bash
cd /projects/rust/chaos
git add crates/chaos-server/src/notify.rs crates/chaos-server/src/monitor.rs
git commit -m "$(cat <<'EOF'
feat(server): service down/recovered alerts from the monitor

Pure AlertTracker per service: Down/Degraded alert after 2 consecutive
sweeps (flap debounce), Paused/Starting/Unknown are neutral; sends
happen after the statuses lock is released.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

---

## Task 4: calendar reminders — pure window/dedup + background task

**Files:**
- `crates/chaos-server/src/notify.rs` (pure fns, `ReminderLog`, task)
- `crates/chaos-server/src/main.rs` (spawn)

### Steps

- [ ] Add failing unit tests to the `tests` module in `crates/chaos-server/src/notify.rs` (add `use chrono::{Duration as ChronoDuration, TimeZone, Utc};` and `use uuid::Uuid;` to the test imports):

```rust
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
```

- [ ] Run `cargo nextest run -p chaos-server notify` — compile error (`due`, `ReminderLog` unknown). Red.
- [ ] Implement the pure layer in `crates/chaos-server/src/notify.rs` (add top-level imports `use std::collections::HashSet;`, `use chrono::{DateTime, Utc};`, `use uuid::Uuid;`):

```rust
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
```

- [ ] Run `cargo nextest run -p chaos-server notify` — green.
- [ ] Add the background task to `crates/chaos-server/src/notify.rs` (add imports `use std::time::Duration;` — already there — plus `use chaos_domain::CalendarKind;` and `use crate::state::AppState;`; keep `Notifier` import ordering clippy-clean):

```rust
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
                send_reminder(notifier, &event.title, event.starts_at, event.location.as_deref(), now)
                    .await;
            }
        }

        let calendars = state.db.list_calendars(user.id).await.map_err(|e| e.to_string())?;
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
```

- [ ] Spawn it in `crates/chaos-server/src/main.rs`, right after the existing spawns:

```rust
    monitor::spawn(state.clone());
    archiver::spawn(state.clone());
    if state.notifier.is_some() && state.config.notifications.calendar_reminders {
        notify::spawn_reminders(state.clone());
    }
```

- [ ] Run `cargo nextest run -p chaos-server` — full crate green (`due`/`ReminderLog` tests plus everything existing).
- [ ] Run `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] Commit:

```bash
cd /projects/rust/chaos
git add crates/chaos-server/src/notify.rs crates/chaos-server/src/main.rs
git commit -m "$(cat <<'EOF'
feat(server): calendar reminders via ntfy

Minutely scanner over every user's local events and ICS feeds; pure
due()/ReminderLog window+dedup (in-memory, pruned), all-day events
skipped, relative-time messages. Spawned only when configured.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

---

## Task 5: example config, deployment docs, roadmap

**Files:**
- `crates/chaos-server/chaos.example.toml`
- `docs/deployment.md`
- `docs/ROADMAP.md`

### Steps

- [ ] Append to `crates/chaos-server/chaos.example.toml` (after the `[home_assistant]` block, same commented-out style):

```toml

# Push notifications via ntfy (https://ntfy.sh or self-hosted). Omit the
# whole section to disable. Service alerts fire on down/recovered
# transitions (debounced over 2 checks); calendar reminders ping shortly
# before events start (local calendars + ICS feeds, all users, one topic).
# [notifications]
# ntfy_url = "https://ntfy.sh"
# topic = "chaos-zeus"
# token = "tk_..."              # only for protected topics
# service_alerts = true
# calendar_reminders = true
# reminder_lead_minutes = 15
```

- [ ] Add a section to `docs/deployment.md` (after "Controlling systemd units", before "Desktop and phone apps"):

```markdown
## Notifications (ntfy)

chaos publishes to an [ntfy](https://ntfy.sh) topic — subscribe with the
ntfy phone app or web UI. Two kinds of pings, both server-side (no web
push, no browser permission dance):

- **Service alerts**: a monitored service going Down/Degraded (or
  recovering) notifies once, after the state survived two polling sweeps —
  flapping services stay silent.
- **Calendar reminders**: events starting within `reminder_lead_minutes`
  (local calendars and ICS feeds, every user) notify once per occurrence.
  All-day events are skipped.

`settings` is free-form, so it is just more TOML:

```nix
services.chaos.settings.notifications = {
  ntfy_url = "https://ntfy.sh";   # or a self-hosted instance
  topic = "chaos-zeus";
  # token = "tk_...";             # only for protected topics
  reminder_lead_minutes = 15;
};
```

Omit the section to keep notifications off. `service_alerts` and
`calendar_reminders` (both default `true`) toggle the halves
independently.
```

- [ ] Update `docs/ROADMAP.md` Phase 8: change

```markdown
- [ ] Notifications: service down / calendar reminders via ntfy or web push
```

  to

```markdown
- [x] Notifications: service down/recovered alerts (flap-debounced) +
      calendar reminders via ntfy (`[notifications]` in chaos.toml; web
      push not needed — ntfy app covers phones)
```

- [ ] Verify: `cargo nextest run --workspace` still green (docs-only task, but run it — it is the pre-commit convention) and the example config still parses:

```bash
cd /projects/rust/chaos && CHAOS_CONFIG=crates/chaos-server/chaos.example.toml cargo run -p chaos-server -- list-users
```

  (expect it to start, load config, print the user list or a DB path error — not a config parse error; delete any stray `chaos.db` it creates only if it did).

- [ ] Commit:

```bash
cd /projects/rust/chaos
git add crates/chaos-server/chaos.example.toml docs/deployment.md docs/ROADMAP.md
git commit -m "$(cat <<'EOF'
docs: ntfy notifications — example config, deployment notes, roadmap

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

---

## Final verification

- [ ] `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo nextest run --workspace` — all green (this is `just check` + `just test`).
- [ ] Grep sanity: `grep -rn "notifier\|Notifier" crates/chaos-server/src --include='*.rs'` shows config, state, monitor, notify, main and nothing else.
- [ ] Confirm zero-config path: `Config::default().notifications.ntfy_url.is_none()`, `AppState.notifier == None`, no reminder task spawned, monitor behavior byte-identical (alerting flag false).
