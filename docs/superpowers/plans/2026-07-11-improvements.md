# Review Improvements Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Harden the server (login timing oracle + throttle, Secure cookie flag, event-update scoping, input length caps), speed up the merged calendar view and the Leptos UI (parallel ICS fetches, search debounce, keyed iteration, shared HTTP client), make dialogs accessible, and close review-flagged test gaps.

**Architecture:** Server changes stay inside the existing module boundaries: `crates/chaos-server/src/auth.rs` gains the throttle + dummy-verify primitives, handlers only wire them; validation caps live next to the existing `validate_*` helpers in `db.rs`/`db_calendar.rs`. UI changes are local to `chaos-ui`: a debounced query `Memo` in the links page, `<For>` in three list renders, one context-provided `ChaosClient`, and one accessible `Modal` shared by every dialog. Every task is TDD where the logic is pure; wasm-only wiring is verified with `cargo check -p chaos-ui`.

**Tech Stack:** Rust 2024 workspace — Axum 0.8 + sqlx/SQLite + argon2 (server), Leptos 0.8 CSR + reqwest wasm backend (UI), figment (config), futures (`join_all`).

---

## Note for the executor: locating code after earlier refactors

This plan was written against the pre-refactor tree. Earlier refactor plans may have:

- moved handlers out of `crates/chaos-server/src/api/mod.rs` into `api/services.rs` + `api/widgets.rs`,
- introduced `crates/chaos-ui/src/hooks.rs` (it does not exist at time of writing).

**All transformations below are described relative to functions, not line numbers.** Before each task, locate the named function with `grep -rn "fn <name>" crates/<crate>/src` and apply the change wherever it now lives. If `crates/chaos-ui/src/hooks.rs` exists, put the new UI helpers (`debounce_signal`, and the `SharedClient` context if lib.rs was split) there instead of the files named below, keeping the same code. If a cited snippet no longer matches exactly, re-read the function and adapt the edit to its current shape — the intent of each change is stated in the task intro.

Run `cargo fmt` before every commit. Commit messages follow the repo's conventional-commit style (`feat(...)`, `fix(...)`, `test(...)`) and **must** end with both trailers shown in each commit step.

---

### Task 1: Login hardening — constant-cost verification + failed-attempt throttle

Unknown-user logins currently return immediately while known users pay ~100 ms of argon2 — a username-enumeration timing oracle — and nothing slows brute force. Fix both: on user-miss, verify the password against a lazily generated dummy argon2 hash (same cost both paths); add an in-memory per-`username|ip` exponential-backoff throttle. No new crates: `argon2`, `tokio` (with `time`, already used by `monitor.rs`), and `std::sync` cover everything.

**Files:**
- Modify: `crates/chaos-server/src/auth.rs` (primitives + tests)
- Modify: `crates/chaos-server/src/api/auth.rs` (the `login` handler — locate `pub async fn login`)
- Modify: `crates/chaos-server/src/state.rs` (`AppState` field)

- [ ] **Step 1: Write the failing tests**

Append to the existing `#[cfg(test)] mod tests` in `crates/chaos-server/src/auth.rs`:

```rust
    #[test]
    fn unknown_user_still_pays_a_password_verification() {
        // The dummy hash is a real argon2 PHC string, so the user-miss path
        // costs the same as a real verification.
        assert!(PasswordHash::new(dummy_hash()).is_ok());
        assert!(!verify_login(None, "hunter2"));

        let hash = hash_password("hunter2").expect("hash");
        assert!(verify_login(Some(&hash), "hunter2"));
        assert!(!verify_login(Some(&hash), "wrong"));
    }

    #[test]
    fn throttle_delay_backs_off_after_free_failures() {
        use std::time::Duration;
        assert_eq!(throttle_delay(0), Duration::ZERO);
        assert_eq!(throttle_delay(2), Duration::ZERO);
        assert_eq!(throttle_delay(3), Duration::from_millis(500));
        assert_eq!(throttle_delay(4), Duration::from_secs(1));
        assert_eq!(throttle_delay(5), Duration::from_secs(2));
        // Capped, even for absurd counts.
        assert_eq!(throttle_delay(60), Duration::from_secs(30));
    }

    #[test]
    fn failed_attempts_are_tracked_per_key_and_cleared_on_success() {
        use std::time::Duration;
        let throttle = LoginThrottle::default();
        assert_eq!(throttle.delay("tibo|1.2.3.4"), Duration::ZERO);
        for _ in 0..3 {
            throttle.record_failure("tibo|1.2.3.4");
        }
        assert_eq!(throttle.delay("tibo|1.2.3.4"), Duration::from_millis(500));
        // Another user/IP pair is unaffected.
        assert_eq!(throttle.delay("tibo|5.6.7.8"), Duration::ZERO);
        throttle.clear("tibo|1.2.3.4");
        assert_eq!(throttle.delay("tibo|1.2.3.4"), Duration::ZERO);
    }

    #[test]
    fn throttle_key_normalizes_username_and_defaults_ip() {
        assert_eq!(throttle_key(" Tibo ", Some("1.2.3.4")), "tibo|1.2.3.4");
        assert_eq!(throttle_key("tibo", None), "tibo|unknown");
    }

    #[test]
    fn client_ip_prefers_forwarded_for() {
        let mut headers = axum::http::HeaderMap::new();
        assert_eq!(client_ip(&headers), None);
        headers.insert("x-real-ip", "10.0.0.9".parse().unwrap());
        assert_eq!(client_ip(&headers).as_deref(), Some("10.0.0.9"));
        headers.insert("x-forwarded-for", "1.2.3.4, 10.0.0.1".parse().unwrap());
        assert_eq!(client_ip(&headers).as_deref(), Some("1.2.3.4"));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p chaos-server --lib auth::tests`
Expected: compile error — `dummy_hash`, `verify_login`, `throttle_delay`, `LoginThrottle`, `throttle_key`, `client_ip` not found.

- [ ] **Step 3: Implement the primitives in `crates/chaos-server/src/auth.rs`**

Add to the imports at the top of the file:

```rust
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
```

Add after `verify_password` (keep `verify_password` unchanged):

```rust
/// Verify a login attempt. On a user miss (`stored_hash` is `None`) the
/// password is still verified — against a dummy hash — so the unknown-user
/// path costs the same ~100 ms of argon2 as the known-user path and login
/// timing does not reveal which usernames exist.
pub fn verify_login(stored_hash: Option<&str>, password: &str) -> bool {
    match stored_hash {
        Some(hash) => verify_password(password, hash),
        None => {
            let _ = verify_password(password, dummy_hash());
            false
        }
    }
}

/// A valid argon2 PHC hash of a random token, generated once per process.
/// Nothing can ever match it; it only exists to burn verification time.
fn dummy_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| hash_password(&new_token()).expect("hashing dummy password"))
}

// ---- failed-login throttle ----

/// Failures a `username|ip` pair gets before delays kick in.
const THROTTLE_FREE_FAILURES: u32 = 3;
const THROTTLE_BASE_DELAY: Duration = Duration::from_millis(500);
const THROTTLE_MAX_DELAY: Duration = Duration::from_secs(30);
/// A pair with no failures for this long is forgotten.
const THROTTLE_RESET: Duration = Duration::from_secs(15 * 60);

/// Delay owed after `failures` consecutive failures: nothing for the first
/// few (typos), then exponential backoff capped at [`THROTTLE_MAX_DELAY`].
fn throttle_delay(failures: u32) -> Duration {
    if failures < THROTTLE_FREE_FAILURES {
        return Duration::ZERO;
    }
    let exponent = (failures - THROTTLE_FREE_FAILURES).min(6);
    (THROTTLE_BASE_DELAY * 2u32.pow(exponent)).min(THROTTLE_MAX_DELAY)
}

/// In-memory failed-login tracker, keyed by `username|ip` (see
/// [`throttle_key`]). Single-instance servers only need process memory:
/// a restart forgiving old failures is fine.
#[derive(Default)]
pub struct LoginThrottle {
    attempts: Mutex<HashMap<String, (u32, Instant)>>,
}

impl LoginThrottle {
    /// How long this attempt must wait before being processed.
    pub fn delay(&self, key: &str) -> Duration {
        let mut attempts = self.attempts.lock().expect("throttle lock");
        match attempts.get(key) {
            Some((failures, last)) if last.elapsed() < THROTTLE_RESET => {
                throttle_delay(*failures)
            }
            Some(_) => {
                attempts.remove(key);
                Duration::ZERO
            }
            None => Duration::ZERO,
        }
    }

    pub fn record_failure(&self, key: &str) {
        let mut attempts = self.attempts.lock().expect("throttle lock");
        let entry = attempts.entry(key.to_string()).or_insert((0, Instant::now()));
        entry.0 = entry.0.saturating_add(1);
        entry.1 = Instant::now();
    }

    /// Successful login: the pair starts fresh.
    pub fn clear(&self, key: &str) {
        self.attempts.lock().expect("throttle lock").remove(key);
    }
}

pub fn throttle_key(username: &str, ip: Option<&str>) -> String {
    format!(
        "{}|{}",
        username.trim().to_lowercase(),
        ip.unwrap_or("unknown")
    )
}

/// Best-effort client address for throttling: the reverse proxy's
/// `X-Forwarded-For` (first hop) or `X-Real-IP`. Spoofable by direct LAN
/// clients, but combined with the username key it still stops dumb loops.
pub fn client_ip(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|ip| ip.trim().to_string())
        .filter(|ip| !ip.is_empty())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|ip| ip.trim().to_string())
        })
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p chaos-server --lib auth::tests`
Expected: all tests PASS (including the pre-existing `password_roundtrip`).

- [ ] **Step 5: Wire the throttle into `AppState`**

In `crates/chaos-server/src/state.rs`, add the field to `AppState` (after `home`):

```rust
    /// Failed-login backoff tracker (in-memory, per username+IP).
    pub login_throttle: Arc<crate::auth::LoginThrottle>,
```

and in `AppState::new`, add to the `Self { ... }` literal:

```rust
            login_throttle: Arc::new(crate::auth::LoginThrottle::default()),
```

- [ ] **Step 6: Rewrite the `login` handler**

Locate `pub async fn login` (currently `crates/chaos-server/src/api/auth.rs`). Replace the lookup + verify block (the `let (user, stored_hash) = ...` through the `if !verify_password(...)` return) with the version below, and add `headers: HeaderMap` as the second extractor (before `Json`, which must stay last). Update the imports in that file to pull the new items from `crate::auth`:

```rust
use crate::auth::{
    AuthUser, SESSION_COOKIE, SESSION_DAYS, client_ip, new_token, request_token, throttle_key,
    token_hash, verify_login,
};
```

```rust
pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    // Repeated failures for the same username+IP earn an increasing delay.
    let key = throttle_key(&req.username, client_ip(&headers).as_deref());
    let delay = state.login_throttle.delay(&key);
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }

    // Same rejection — and the same argon2 cost — for unknown user and
    // wrong password (see verify_login).
    let found = state.db.user_with_password(&req.username).await.ok();
    if !verify_login(found.as_ref().map(|(_, hash)| hash.as_str()), &req.password) {
        state.login_throttle.record_failure(&key);
        return Err(ApiError::Unauthorized);
    }
    let (user, _) = found.expect("verify_login returns false when the user is missing");
    state.login_throttle.clear(&key);

    let token = new_token();
    // ... rest of the function unchanged (create_session, tracing, response).
```

(The `verify_password` import in this file becomes unused — remove it.)

- [ ] **Step 7: Verify the whole crate is green**

Run: `cargo test -p chaos-server`
Expected: PASS, no warnings from `cargo check -p chaos-server`.

- [ ] **Step 8: Commit**

```bash
cd /projects/rust/chaos
git add crates/chaos-server/src/auth.rs crates/chaos-server/src/api/auth.rs crates/chaos-server/src/state.rs
git commit -m "$(cat <<'EOF'
feat(server): equalize login timing and throttle failed attempts

Unknown-user logins now verify against a dummy argon2 hash so the
user-miss path costs the same as a real verification (no username
enumeration by timing), and repeated failures per username+IP earn an
exponential in-memory backoff.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

---

### Task 2: `secure_cookies` config option

The session cookie never carries `Secure`, so a TLS deployment leaks it over any accidental http request. Add a flat `secure_cookies = false` (default) option to the server config; when true, append `; Secure` to the session cookie.

**Files:**
- Modify: `crates/chaos-server/src/config.rs`
- Modify: `crates/chaos-server/src/api/auth.rs` (locate `fn session_cookie_headers` and its callers `login`/`logout`)

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/chaos-server/src/config.rs`:

```rust
    #[test]
    fn secure_cookies_defaults_off_and_parses() {
        let config = super::Config::default();
        assert!(!config.secure_cookies);

        let config: super::Config = figment::Figment::from(
            figment::providers::Serialized::defaults(super::Config::default()),
        )
        .merge(figment::providers::Toml::string("secure_cookies = true"))
        .extract()
        .expect("secure_cookies must parse");
        assert!(config.secure_cookies);
    }
```

Add a `#[cfg(test)] mod tests` to `crates/chaos-server/src/api/auth.rs` (at the end of the file):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header;

    fn cookie(headers: &HeaderMap) -> &str {
        headers
            .get(header::SET_COOKIE)
            .expect("set-cookie present")
            .to_str()
            .expect("ascii cookie")
    }

    #[test]
    fn session_cookie_secure_flag_follows_config() {
        let plain = session_cookie_headers("tok", 60, false);
        assert!(!cookie(&plain).contains("Secure"));
        assert!(cookie(&plain).contains("HttpOnly"));

        let secure = session_cookie_headers("tok", 60, true);
        assert!(cookie(&secure).ends_with("; Secure"));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p chaos-server --lib config::tests api::auth::tests`
Expected: compile errors — no field `secure_cookies`, `session_cookie_headers` takes 2 arguments.

- [ ] **Step 3: Implement**

`config.rs` — add to `struct Config` (after `home_assistant`):

```rust
    /// Append `Secure` to the session cookie. Turn on when the server is
    /// reached over HTTPS (reverse proxy with TLS); off by default because
    /// plain-http LAN setups would otherwise lose their session cookie.
    pub secure_cookies: bool,
```

and to `impl Default for Config`:

```rust
            secure_cookies: false,
```

`api/auth.rs` — change `session_cookie_headers` and its two callers:

```rust
fn session_cookie_headers(token: &str, max_age_secs: i64, secure: bool) -> HeaderMap {
    let mut cookie =
        format!("{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_secs}");
    if secure {
        cookie.push_str("; Secure");
    }
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        headers.insert(header::SET_COOKIE, value);
    }
    headers
}
```

In `login`: `session_cookie_headers(&token, SESSION_DAYS * 24 * 60 * 60, state.config.secure_cookies)`.
In `logout`: `session_cookie_headers("", 0, state.config.secure_cookies)`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p chaos-server`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cd /projects/rust/chaos
git add crates/chaos-server/src/config.rs crates/chaos-server/src/api/auth.rs
git commit -m "$(cat <<'EOF'
feat(server): secure_cookies config flag for the session cookie

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

---

### Task 3: Fetch ICS feeds in parallel in the merged events view

Locate `pub async fn events` in the calendar API module (currently `crates/chaos-server/src/api/calendar.rs`). It loops over ICS calendars sequentially, so N broken feeds cost N×15 s. Fetch them concurrently with `futures::future::join_all`, mirroring the `sensors` handler in `api/home.rs`.

**Note:** `ics.rs` tests are pure `parse`/`expand` tests — there is **no** stub-feed HTTP harness in the crate, so per the spec no new test is added here; the existing suite must stay green.

**Files:**
- Modify: `crates/chaos-server/src/api/calendar.rs` (function `events`)

- [ ] **Step 1: Replace the sequential loop**

Replace the whole `for calendar in state.db.list_calendars(user.id).await? { ... }` block inside `events` with:

```rust
    // ICS feeds are fetched concurrently: a broken feed costs one timeout,
    // not one timeout per feed (same pattern as api/home.rs sensors).
    let feeds: Vec<_> = state
        .db
        .list_calendars(user.id)
        .await?
        .into_iter()
        .filter(|calendar| calendar.kind == CalendarKind::Ics && calendar.ics_url.is_some())
        .collect();
    let results = futures::future::join_all(feeds.iter().map(|calendar| {
        state.ics.events(
            calendar.id,
            calendar.ics_url.as_deref().expect("filtered above"),
            query.start,
            query.end,
        )
    }))
    .await;
    for (calendar, result) in feeds.into_iter().zip(results) {
        match result {
            Ok(feed_events) => out.extend(feed_events.into_iter().map(|event| CalendarEvent {
                id: None,
                calendar_id: calendar.id,
                calendar_name: calendar.name.clone(),
                color: calendar.color.clone(),
                title: event.title,
                description: event.description,
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
```

- [ ] **Step 2: Verify everything still builds and passes**

Run: `cargo test -p chaos-server && cargo check -p chaos-server`
Expected: PASS, no warnings.

- [ ] **Step 3: Commit**

```bash
cd /projects/rust/chaos
git add crates/chaos-server/src/api/calendar.rs
git commit -m "$(cat <<'EOF'
perf(server): fetch ICS feeds concurrently in the merged events view

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

---

### Task 4: Scope `update_event`'s UPDATE by owner (defense in depth)

`Db::update_event`'s final `UPDATE events ... WHERE id = ?` relies entirely on the preceding `get_event` ownership check. Scope the SQL itself like `delete_event` does and check `rows_affected`.

**Files:**
- Modify: `crates/chaos-server/src/db_calendar.rs` (function `update_event` + tests)

- [ ] **Step 1: Extend the failing test**

In `db_calendar.rs`'s `event_crud_scoped_by_user` test, after the existing cross-user `delete_event` assertion, add:

```rust
        // Another user cannot update it either — both the pre-check and the
        // UPDATE's own scoping must refuse.
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
            db.get_event(user_id, event.id).await.expect("still ours").title,
            "Dentist"
        );
```

- [ ] **Step 2: Run it**

Run: `cargo test -p chaos-server --lib db_calendar::tests::event_crud_scoped_by_user`
Expected: PASS already (the `get_event` pre-check catches it) — this documents the contract. The SQL change is defense in depth and must not regress it.

- [ ] **Step 3: Scope the UPDATE**

In `update_event`, replace the query execution with:

```rust
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
```

- [ ] **Step 4: Run the suite**

Run: `cargo test -p chaos-server`
Expected: PASS (including the update path in existing tests).

- [ ] **Step 5: Commit**

```bash
cd /projects/rust/chaos
git add crates/chaos-server/src/db_calendar.rs
git commit -m "$(cat <<'EOF'
fix(server): scope update_event's UPDATE by calendar owner

Defense in depth: the SQL itself now refuses cross-user writes instead
of relying only on the get_event pre-check, mirroring delete_event.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

---

### Task 5: Input length caps

`validate_name` (db.rs), `validate_calendar`/`validate_event` (db_calendar.rs) only check non-emptiness — a client can store megabytes per field. Cap names/titles/locations at 512 chars, descriptions/URLs at 4096.

**Files:**
- Modify: `crates/chaos-server/src/db.rs` (helper + `validate_name` + `create_link`/`update_link`/collection functions + tests)
- Modify: `crates/chaos-server/src/db_calendar.rs` (`validate_calendar`, `validate_event` + tests)

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `db.rs`:

```rust
    #[tokio::test]
    async fn oversized_inputs_are_refused() {
        let db = Db::in_memory().await.unwrap();
        let long_name = "x".repeat(MAX_NAME_LEN + 1);
        let long_text = "x".repeat(MAX_TEXT_LEN + 1);

        assert!(matches!(
            db.create_collection(&CollectionRequest {
                name: long_name.clone(),
                description: None,
                color: None,
                parent_id: None,
            })
            .await,
            Err(DbError::Constraint(_))
        ));

        assert!(matches!(
            db.create_link(
                &CreateLinkRequest {
                    url: "https://example.com".parse().unwrap(),
                    title: Some(long_name.clone()),
                    description: None,
                    collection_id: None,
                    tags: vec![],
                },
                false,
                None,
            )
            .await,
            Err(DbError::Constraint(_))
        ));

        assert!(matches!(
            db.create_link(
                &CreateLinkRequest {
                    url: "https://example.com".parse().unwrap(),
                    title: None,
                    description: Some(long_text.clone()),
                    collection_id: None,
                    tags: vec![],
                },
                false,
                None,
            )
            .await,
            Err(DbError::Constraint(_))
        ));

        // At the limit everything still works.
        let ok = db
            .create_collection(&CollectionRequest {
                name: "x".repeat(MAX_NAME_LEN),
                description: Some("y".repeat(MAX_TEXT_LEN)),
                color: None,
                parent_id: None,
            })
            .await;
        assert!(ok.is_ok());
    }
```

(If the tests module lacks these imports, they are already used by neighboring tests — `CollectionRequest`, `CreateLinkRequest` come via `chaos_domain`; add `use chaos_domain::{CollectionRequest, CreateLinkRequest};` inside the module if needed.)

Append to `mod tests` in `db_calendar.rs`:

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p chaos-server --lib oversized`
Expected: `db.rs` test fails to compile (`MAX_NAME_LEN` undefined); after adding the constants it fails with `Ok(...)` where `Err(Constraint)` is expected. The `db_calendar` test FAILS (creates succeed).

- [ ] **Step 3: Implement the caps**

In `db.rs`, next to `validate_name`:

```rust
/// Sanity caps on free-text inputs — generous for humans, hostile to blobs.
pub(crate) const MAX_NAME_LEN: usize = 512;
pub(crate) const MAX_TEXT_LEN: usize = 4096;

pub(crate) fn validate_len(field: &str, value: &str, max: usize) -> Result<()> {
    if value.chars().count() > max {
        return Err(DbError::Constraint(format!(
            "{field} is too long (max {max} characters)"
        )));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(DbError::Constraint("name must not be empty".into()));
    }
    validate_len("name", name, MAX_NAME_LEN)
}
```

Still in `db.rs`, add description/URL/title checks to the four write paths (top of each function, before any query):

- `create_collection` and `update_collection` (both already call `validate_name(&req.name)?;` — add after it):

```rust
        if let Some(description) = &req.description {
            validate_len("description", description, MAX_TEXT_LEN)?;
        }
```

- `create_link` (start of the function body):

```rust
        if let Some(title) = &req.title {
            validate_len("title", title, MAX_NAME_LEN)?;
        }
        if let Some(description) = &req.description {
            validate_len("description", description, MAX_TEXT_LEN)?;
        }
        validate_len("url", req.url.as_str(), MAX_TEXT_LEN)?;
```

- `update_link` (after the existing `validate_name(&req.title)?;`):

```rust
        if let Some(description) = &req.description {
            validate_len("description", description, MAX_TEXT_LEN)?;
        }
        validate_len("url", req.url.as_str(), MAX_TEXT_LEN)?;
```

In `db_calendar.rs`, import the helpers — change the existing `use crate::db::{Db, DbError, Result};` to:

```rust
use crate::db::{Db, DbError, MAX_NAME_LEN, MAX_TEXT_LEN, Result, validate_len};
```

(`validate_len`, `MAX_NAME_LEN`, `MAX_TEXT_LEN` must be `pub(crate)` in db.rs — they are, per the snippet above.) Then extend the validators:

```rust
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
```

- [ ] **Step 4: Run the suite**

Run: `cargo test -p chaos-server`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cd /projects/rust/chaos
git add crates/chaos-server/src/db.rs crates/chaos-server/src/db_calendar.rs
git commit -m "$(cat <<'EOF'
feat(server): cap free-text input lengths (names 512, text/urls 4096)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

---

### Task 6: Links page — debounced search, one fetch per change

`pages/links.rs` feeds `search.get()` straight into the resource (one HTTP fetch per keystroke), and a filter change fetches twice (stale page index, then the reset effect fires a second fetch). Fix with a ~250 ms debounce and a `Memo<LinkQuery>` that derives the page reset, so one change produces exactly one query value (`Memo` dedupes on `PartialEq`).

**Files:**
- Modify: `crates/chaos-ui/src/pages/links.rs` (component `Links` + new helpers + tests)
- If `crates/chaos-ui/src/hooks.rs` exists, put `debounce_signal` there (as `pub(crate)`) and import it; otherwise keep it in `links.rs`.

- [ ] **Step 1: Write the failing test for the pure query derivation**

Append at the end of `links.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_query_keeps_page_for_same_filters() {
        let first = effective_query(None, None, Some("rust".into()), None, 2);
        assert_eq!(first.offset, Some(2 * PAGE_SIZE));

        let next = effective_query(Some(&first), None, Some("rust".into()), None, 3);
        assert_eq!(next.offset, Some(3 * PAGE_SIZE));
        assert_eq!(next.limit, Some(PAGE_SIZE));
    }

    #[test]
    fn effective_query_resets_page_when_any_filter_changes() {
        let base = effective_query(None, None, None, None, 4);
        assert_eq!(base.offset, Some(4 * PAGE_SIZE));

        let by_tag = effective_query(Some(&base), None, Some("rust".into()), None, 4);
        assert_eq!(by_tag.offset, Some(0));

        let by_search = effective_query(Some(&base), None, None, Some("query".into()), 4);
        assert_eq!(by_search.offset, Some(0));

        // Note: chaos-ui's uuid dep has no `v7` feature, so build one from
        // raw bytes instead of Uuid::now_v7().
        let by_collection =
            effective_query(Some(&base), Some(Uuid::from_u128(1)), None, None, 4);
        assert_eq!(by_collection.offset, Some(0));
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p chaos-ui effective_query`
Expected: compile error — `effective_query` not found.

- [ ] **Step 3: Implement the pure derivation + debounce helper**

Add above the `Links` component in `links.rs`:

```rust
/// The list query for the current filters and page. When the filters differ
/// from the previous query the page index is discarded — a filter change
/// must show the first page. Deriving that here (instead of an effect that
/// resets the page after the fact) means one filter change produces exactly
/// one query value instead of a stale-page fetch followed by a reset fetch.
fn effective_query(
    prev: Option<&LinkQuery>,
    collection_id: Option<Uuid>,
    tag: Option<String>,
    q: Option<String>,
    page_index: u32,
) -> LinkQuery {
    let filters_changed = prev
        .is_some_and(|p| p.collection_id != collection_id || p.tag != tag || p.q != q);
    let offset = if filters_changed { 0 } else { page_index * PAGE_SIZE };
    LinkQuery {
        collection_id,
        tag,
        q,
        limit: Some(PAGE_SIZE),
        offset: Some(offset),
    }
}

/// A read-only signal that follows `source` once it has been stable for
/// `delay` (a trailing debounce): typing in the search box only queries the
/// server after the user pauses.
fn debounce_signal(source: RwSignal<String>, delay: Duration) -> Signal<String> {
    let out = RwSignal::new(source.get_untracked());
    let generation = StoredValue::new(0u64);
    Effect::new(move |_| {
        let value = source.get();
        let current = generation.with_value(|g| *g + 1);
        generation.set_value(current);
        set_timeout(
            move || {
                if generation.get_value() == current {
                    out.set(value);
                }
            },
            delay,
        );
    });
    out.into()
}
```

- [ ] **Step 4: Rewire the `Links` component**

Inside `pub fn Links()`:

1. Rename the raw input signal and derive the debounced one:

```rust
    let search_input = RwSignal::new(String::new());
    let search = debounce_signal(search_input, Duration::from_millis(250));
```

2. Delete the old `filters` `Memo` and its page-reset `Effect` entirely.

3. Replace the `links` resource with a memoized query:

```rust
    let query = Memo::new(move |prev: Option<&LinkQuery>| {
        effective_query(
            prev,
            selected_collection.get(),
            selected_tag.get(),
            Some(search.get()).filter(|q| !q.trim().is_empty()),
            page_index.get(),
        )
    });
    // Keep the pager display honest after a filter change discarded the page.
    Effect::new(move |_| {
        if query.get().offset == Some(0) {
            page_index.set(0);
        }
    });

    let client = use_client();
    let links = LocalResource::new({
        let client = client.clone();
        move || {
            refresh.track();
            let query = query.get();
            let client = client.clone();
            async move { client.list_links(&query).await }
        }
    });
```

(This replaces the old inline `LinkQuery { ... }` construction. The sync effect cannot loop: setting `page_index` re-runs the memo, which produces an equal `LinkQuery`, so the memo does not notify again.)

4. In the search `<input>`, bind the raw signal:

```rust
                        prop:value=search_input
                        on:input=move |ev| search_input.set(event_target_value(&ev))
```

- [ ] **Step 5: Verify**

Run: `cargo test -p chaos-ui && cargo check -p chaos-ui`
Expected: tests PASS, check clean. Note: the debounce itself is timer-driven wasm behavior and is covered by `cargo check` only.

- [ ] **Step 6: Commit**

```bash
cd /projects/rust/chaos
git add crates/chaos-ui/src/pages/links.rs
git commit -m "$(cat <<'EOF'
perf(ui): debounce link search and fetch once per filter change

The list query is now a Memo deriving the page reset (deduped by
PartialEq), and search input is trailing-debounced by 250ms, so a
keystroke or filter click costs one request instead of two-per-change
and one-per-key.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

---

### Task 7: Keyed `<For>` iteration for services grid, links list, weather rows

Three list renders use `.map(...).collect_view()`, so every refresh rebuilds every DOM node. Switch them to keyed `<For>` so unchanged rows are reused. (`For` is re-exported by `leptos::prelude`.) Note: the review cited "dashboard.rs:105-113" for the services grid — the actual render lives in `ServiceGrid` in `crates/chaos-ui/src/components.rs`; the dashboard page only hosts it.

**Files:**
- Modify: `crates/chaos-ui/src/components.rs` (component `ServiceGrid`)
- Modify: `crates/chaos-ui/src/pages/links.rs` (component `LinkList`)
- Modify: `crates/chaos-ui/src/pages/weather.rs` (the `places` render in `WeatherPage`)

- [ ] **Step 1: ServiceGrid**

Locate `pub fn ServiceGrid` in `components.rs` and replace its view body:

```rust
    view! {
        <div class="service-grid">
            <For
                each=move || services.clone()
                key=|service| service.def.id.clone()
                children=move |service| view! { <ServiceCard service controls/> }
            />
        </div>
    }
    .into_any()
```

- [ ] **Step 2: LinkList**

Locate `fn LinkList` in `pages/links.rs` and replace its view:

```rust
    view! {
        <ul class="link-list">
            <For
                each=move || links.clone()
                key=|link| link.id
                children=move |link| view! { <LinkItem link editing refresh/> }
            />
        </ul>
    }
    .into_any()
```

- [ ] **Step 3: Weather rows**

Locate the `{move || { let list = places.get(); ... }}` block in `WeatherPage` (`pages/weather.rs`) and replace the non-empty branch (`list.into_iter().map(...).collect_view().into_any()`) with:

```rust
                    view! {
                        <For
                            each=move || places.get()
                            key=|place| place.clone()
                            children=move |place| {
                                view! {
                                    <WeatherRow
                                        location=Some(place)
                                        on_remove=Some(remove)
                                        loaded
                                        combined
                                    />
                                }
                            }
                        />
                    }
                        .into_any()
```

(`remove` is a `Callback`, which is `Copy`; `loaded`/`combined` are `Copy` signals — no clones needed.)

- [ ] **Step 4: Verify**

Run: `cargo check -p chaos-ui && cargo test -p chaos-ui`
Expected: clean check, tests PASS.

- [ ] **Step 5: Commit**

```bash
cd /projects/rust/chaos
git add crates/chaos-ui/src/components.rs crates/chaos-ui/src/pages/links.rs crates/chaos-ui/src/pages/weather.rs
git commit -m "$(cat <<'EOF'
perf(ui): keyed <For> for services grid, links list and weather rows

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

---

### Task 8: One shared `ChaosClient` via Leptos context

`use_client()` builds a fresh `reqwest::Client` on every call. Provide one token-less `ChaosClient` in context at `App`; `use_client()` clones it (cheap — `reqwest::Client` is an `Arc` internally) and applies the current token per call. **Token freshness is preserved**: today the token is baked into the client at `use_client()` time (`chaos-client` sends `self.token` in `check_status`, it does not re-read localStorage per request — verified in `crates/chaos-client/src/lib.rs`), and the new code still reads `stored_token()` on every `use_client()` call, exactly as before. No `chaos-client` changes needed.

**Files:**
- Modify: `crates/chaos-ui/src/lib.rs` (locate `pub fn use_client` and `pub fn App`; if a refactor moved `use_client` to `hooks.rs`, edit it there)

- [ ] **Step 1: Add the context type and provide it in `App`**

In `lib.rs`, near the `Session` struct:

```rust
/// One HTTP client for the whole app, provided as context at `App`.
/// `use_client()` clones it per call (reqwest clients are `Arc`s inside),
/// so components share the connection pool instead of building a new
/// `reqwest::Client` on every call.
#[derive(Clone)]
struct SharedClient(ChaosClient);
```

In `pub fn App`, immediately after `provide_context(config);` (the `config` param is moved into context — clone what's needed first):

```rust
    let api_base = config.api_base.clone();
    provide_context(config);
    provide_context(SharedClient(ChaosClient::new(api_base)));
```

(Replace the existing lone `provide_context(config);` line with these three.)

- [ ] **Step 2: Rewire `use_client`**

```rust
pub fn use_client() -> ChaosClient {
    let config = use_context::<AppConfig>().expect("AppConfig provided by the shell");
    // The token is read per call, not per app: it changes on login/logout
    // and callers must always see the current one (matches the previous
    // behavior, where the whole client was rebuilt per call).
    let token = config.persist_token.then(stored_token).flatten();
    match use_context::<SharedClient>() {
        Some(SharedClient(client)) => client.with_token(token),
        // Components rendered outside App (tests, shells) fall back to a
        // one-off client.
        None => ChaosClient::new(config.api_base).with_token(token),
    }
}
```

(`with_token` takes `self` by value; `client` here is already a clone out of the context, so this compiles as-is.)

- [ ] **Step 3: Verify**

Run: `cargo check -p chaos-ui && cargo test -p chaos-ui && cargo check -p chaos-web --target wasm32-unknown-unknown 2>/dev/null || cargo check -p chaos-web`
Expected: clean. (The wasm check is best-effort — skip if the target isn't installed; native check covers the types.)

- [ ] **Step 4: Commit**

```bash
cd /projects/rust/chaos
git add crates/chaos-ui/src/lib.rs
git commit -m "$(cat <<'EOF'
perf(ui): share one ChaosClient via context instead of per-call builds

use_client() still reads the session token per call, so login/logout
behavior is unchanged; only the reqwest client construction is shared.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

---

### Task 9: Modal accessibility (role, aria-modal, Escape, initial focus)

The shared `Modal` in `crates/chaos-ui/src/components.rs` is a bare `<div>`: invisible to screen readers as a dialog, no keyboard close, focus stays behind the backdrop. Since every dialog in the app goes through this one component, fixing it here covers them all.

**Files:**
- Modify: `crates/chaos-ui/src/components.rs` (component `Modal`)

- [ ] **Step 1: Replace the `Modal` component**

```rust
/// Centered dialog over a click-to-close backdrop. Accessible by
/// construction: announced as a modal dialog, focused on open, closed by
/// Escape — every dialog in the app goes through this component.
#[component]
pub fn Modal(
    title: String,
    #[prop(into)] on_close: Callback<()>,
    children: Children,
) -> impl IntoView {
    let dialog = NodeRef::<leptos::html::Div>::new();

    // Move focus into the dialog when it mounts so keyboard and
    // screen-reader users land inside it.
    Effect::new(move |_| {
        if let Some(el) = dialog.get() {
            let _ = el.focus();
        }
    });

    // Escape closes from anywhere while the dialog is up.
    let escape = window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() == "Escape" {
            on_close.run(());
        }
    });
    on_cleanup(move || escape.remove());

    let label = title.clone();
    view! {
        <div class="modal-backdrop" on:click=move |_| on_close.run(())>
            <div
                class="modal"
                role="dialog"
                aria-modal="true"
                aria-label=label
                tabindex="-1"
                node_ref=dialog
                on:click=|ev| ev.stop_propagation()
            >
                <div class="modal-head">
                    <h3>{title}</h3>
                    <button
                        class="modal-close"
                        aria-label="Close dialog"
                        on:click=move |_| on_close.run(())
                    >
                        "✕"
                    </button>
                </div>
                {children()}
            </div>
        </div>
    }
}
```

(`Callback` is `Copy`, so the three `on_close` captures compile without clones. `ev.key()` on `leptos::ev::keydown` and `el.focus()` need no new web-sys features: leptos itself enables `KeyboardEvent`, and `HtmlElement` is already in chaos-ui's feature list — if the compiler disagrees, add the named feature to the `web-sys` features array in `crates/chaos-ui/Cargo.toml`.)

- [ ] **Step 2: Verify**

Run: `cargo check -p chaos-ui && cargo test -p chaos-ui`
Expected: clean check, tests PASS.

- [ ] **Step 3: Commit**

```bash
cd /projects/rust/chaos
git add crates/chaos-ui/src/components.rs
git commit -m "$(cat <<'EOF'
fix(ui): accessible Modal — dialog role, Escape to close, initial focus

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

---

### Task 10: Close review-flagged test gaps (monitor, releases, feed)

Three pure mappings have zero coverage. Each needs a small extraction so the logic is testable without HTTP/systemd; the extractions must not change behavior.

**Files:**
- Modify: `crates/chaos-server/src/monitor.rs` (extract `unit_status` from `check`, + tests)
- Modify: `crates/chaos-server/src/widgets/releases.rs` (extract `release_tag`, + tests)
- Modify: `crates/chaos-server/src/widgets/feed.rs` (extract `map_entries`, + tests)

- [ ] **Step 1: Write the failing tests**

`monitor.rs` — add at the end of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_states_map_to_service_health() {
        // An active unit still needs the HTTP probe.
        assert!(unit_status("active", "app.service").is_none());

        let starting = unit_status("activating", "app.service").unwrap();
        assert_eq!(starting.state, HealthState::Starting);
        assert!(starting.error.is_none());
        assert_eq!(
            unit_status("reloading", "app.service").unwrap().state,
            HealthState::Starting
        );

        let failed = unit_status("failed", "app.service").unwrap();
        assert_eq!(failed.state, HealthState::Down);
        assert_eq!(failed.error.as_deref(), Some("unit app.service failed"));

        // Stopped on purpose is paused, not an alarm.
        assert_eq!(
            unit_status("inactive", "app.service").unwrap().state,
            HealthState::Paused
        );
        assert_eq!(
            unit_status("deactivating", "app.service").unwrap().state,
            HealthState::Paused
        );

        let odd = unit_status("maintenance", "app.service").unwrap();
        assert_eq!(odd.state, HealthState::Unknown);
        assert_eq!(odd.error.as_deref(), Some("unit app.service is maintenance"));
    }
}
```

`releases.rs` — add at the end:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_tag_prefers_the_tag_url_segment() {
        assert_eq!(
            release_tag(
                Some("https://github.com/leptos-rs/leptos/releases/tag/v0.8.2"),
                Some("v0.8.2 title"),
            ),
            "v0.8.2"
        );
        // No /tag/ segment: fall back to the entry title.
        assert_eq!(
            release_tag(Some("https://github.com/x/y/releases"), Some("v1.0")),
            "v1.0"
        );
        assert_eq!(release_tag(None, Some("v1.0")), "v1.0");
        assert_eq!(release_tag(None, None), "?");
    }
}
```

`feed.rs` — add at the end:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rss_entries_fall_back_on_missing_title_and_link() {
        let rss = r#"<?xml version="1.0"?>
<rss version="2.0"><channel><title>Example Blog</title>
<item><title>First</title><link>https://example.com/a</link>
<pubDate>Wed, 01 Jul 2026 10:00:00 GMT</pubDate></item>
<item><description>no title, no link</description></item>
</channel></rss>"#;
        let feed = feed_rs::parser::parse(rss.as_bytes()).expect("parse rss");
        let items = map_entries(feed);

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "First");
        assert_eq!(items[0].url.as_ref().map(|u| u.as_str()), Some("https://example.com/a"));
        assert_eq!(items[0].source.as_deref(), Some("Example Blog"));
        assert!(items[0].published.is_some());

        assert_eq!(items[1].title, "(untitled)");
        assert!(items[1].url.is_none());
        assert!(items[1].published.is_none());
    }

    #[test]
    fn atom_entries_fall_back_to_updated_when_unpublished() {
        let atom = r#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom"><title>Atom Feed</title>
<entry><title>Only updated</title><updated>2026-07-01T10:00:00Z</updated></entry>
</feed>"#;
        let feed = feed_rs::parser::parse(atom.as_bytes()).expect("parse atom");
        let items = map_entries(feed);

        assert_eq!(items.len(), 1);
        assert!(items[0].published.is_some(), "published falls back to updated");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p chaos-server --lib monitor::tests widgets::releases::tests widgets::feed::tests`
Expected: compile errors — `unit_status`, `release_tag`, `map_entries` not found.

- [ ] **Step 3: Extract the pure functions**

`monitor.rs` — add near `status_only` and rewrite the `Ok` arm of `check`:

```rust
/// Map a systemd ActiveState to a service status, or `None` for "active":
/// an active unit still needs the HTTP probe to prove the app answers.
fn unit_status(active_state: &str, unit: &str) -> Option<ServiceStatus> {
    match active_state {
        "active" => None,
        "activating" | "reloading" => Some(status_only(HealthState::Starting, None)),
        "failed" => Some(status_only(
            HealthState::Down,
            Some(format!("unit {unit} failed")),
        )),
        // inactive / deactivating: stopped on purpose — the default state
        // for on-demand services, so no HTTP check and no alarm.
        "inactive" | "deactivating" => Some(status_only(HealthState::Paused, None)),
        other => Some(status_only(
            HealthState::Unknown,
            Some(format!("unit {unit} is {other}")),
        )),
    }
}
```

and in `check`:

```rust
    match systemd::query(unit).await {
        Ok((active_state, _)) => match unit_status(&active_state, unit) {
            Some(status) => status,
            None => {
                let mut status = probe_http(client, service).await;
                // systemd reports active before slow apps bind their port
                // (JVM services take a while); a running unit with a dead
                // port reads as still starting, not down.
                if status.state == HealthState::Down {
                    status.state = HealthState::Starting;
                }
                status
            }
        },
        Err(reason) => {
            tracing::warn!(unit, reason, "systemd query failed for service");
            status_only(HealthState::Unknown, Some(reason))
        }
    }
```

`releases.rs` — add the function and use it in `latest_release`:

```rust
/// Release links look like …/releases/tag/<tag>; the tag is the cleanest
/// short label, falling back to the release title, then "?".
fn release_tag(link: Option<&str>, title: Option<&str>) -> String {
    link.and_then(|href| href.split("/tag/").nth(1))
        .map(str::to_string)
        .or_else(|| title.map(str::to_string))
        .unwrap_or_else(|| "?".into())
}
```

In `latest_release`, replace the `let tag = link.as_deref()...` chain with:

```rust
    let tag = release_tag(
        link.as_deref(),
        entry.title.as_ref().map(|t| t.content.as_str()),
    );
```

`feed.rs` — extract the mapping from `fetch_one`:

```rust
/// Entry → FeedItem with the documented fallbacks: untitled entries get a
/// placeholder title, unparseable/missing links drop to None, and undated
/// entries fall back to their `updated` stamp.
fn map_entries(feed: feed_rs::model::Feed) -> Vec<FeedItem> {
    let source = feed.title.map(|t| t.content);
    feed.entries
        .into_iter()
        .map(|entry| FeedItem {
            title: entry
                .title
                .map(|t| t.content)
                .unwrap_or_else(|| "(untitled)".into()),
            url: entry.links.first().and_then(|l| l.href.parse().ok()),
            source: source.clone(),
            published: entry.published.or(entry.updated),
            score: None,
            comments: None,
            comments_url: None,
        })
        .collect()
}
```

`fetch_one` then ends with:

```rust
    let feed = feed_rs::parser::parse(body.as_ref()).map_err(|e| e.to_string())?;
    Ok(map_entries(feed))
```

- [ ] **Step 4: Run the suite**

Run: `cargo test -p chaos-server`
Expected: PASS. If a feed_rs assertion surprises (e.g. it synthesizes an entry date), adjust the *assertion* to the library's actual behavior, not the production code — the point is pinning the fallback mapping.

- [ ] **Step 5: Commit**

```bash
cd /projects/rust/chaos
git add crates/chaos-server/src/monitor.rs crates/chaos-server/src/widgets/releases.rs crates/chaos-server/src/widgets/feed.rs
git commit -m "$(cat <<'EOF'
test(server): cover monitor state mapping, release tags, feed fallbacks

Extracts unit_status, release_tag and map_entries as pure functions so
the review-flagged mappings are pinned by unit tests.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

---

## Final verification (after all tasks)

- [ ] Run: `cargo test --workspace` — Expected: PASS
- [ ] Run: `cargo check --workspace` — Expected: no warnings introduced by this plan
- [ ] Run: `cargo fmt --check` — Expected: clean
