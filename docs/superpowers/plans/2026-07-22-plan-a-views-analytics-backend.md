# Plan A — Viewed-State + Analytics Backend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist per-user post engagement (seen / opened-comments / opened-article with first-occurrence timestamps), system post-ingestion timestamps, and a generic analytics event log; expose auth-gated endpoints and typed client calls.

**Architecture:** Three SQLite tables (`post_views`, `posts`, `events`) mirroring the calendar per-user pattern (`AuthUser` handlers + `WHERE user_id = ?`). New `db_views.rs`/`db_posts.rs`/`db_events.rs` `impl Db` blocks. Domain wire types in `chaos-domain`. Server logs `login`/`search` itself and upserts `posts` on the news fetch; the client gets typed calls the UI plan uses.

**Tech Stack:** Axum, sqlx/SQLite, chrono, uuid v7. Spec: `docs/superpowers/specs/2026-07-22-news-viewed-state-analytics-design.md`.

**Verification (every task):**
- `cargo test -p <crate touched>`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`

Commit UNSIGNED (`git -c commit.gpgsign=false`), one per task, each message ending with:
`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
`Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP`
Do NOT push. Leave the two dirty `android-schema.json`/`mobile-schema.json` files untouched.

---

### Task A1: Migration

**Files:**
- Create: `crates/chaos-server/migrations/0005_views_analytics.sql`

- [ ] **Step 1: Write the migration** (verify the highest existing migration number in `crates/chaos-server/migrations/` first; use the next number if not `0005`):

```sql
CREATE TABLE post_views (
    user_id            TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    source             TEXT NOT NULL,
    post_id            TEXT NOT NULL,
    seen_at            TEXT,
    opened_comments_at TEXT,
    opened_article_at  TEXT,
    updated_at         TEXT NOT NULL,
    PRIMARY KEY (user_id, source, post_id)
);
CREATE INDEX idx_post_views_user ON post_views(user_id);

CREATE TABLE posts (
    source        TEXT NOT NULL,
    post_id       TEXT NOT NULL,
    title         TEXT NOT NULL,
    first_seen_at TEXT NOT NULL,
    PRIMARY KEY (source, post_id)
);

CREATE TABLE events (
    id      TEXT PRIMARY KEY,
    user_id TEXT REFERENCES users(id) ON DELETE SET NULL,
    kind    TEXT NOT NULL,
    at      TEXT NOT NULL,
    detail  TEXT
);
CREATE INDEX idx_events_kind_at ON events(kind, at);
CREATE INDEX idx_events_user ON events(user_id, at);
```

- [ ] **Step 2: Verify migrations apply.** Run `cargo test -p chaos-server` (the in-memory test DB runs all migrations on setup; a broken migration fails every DB test). Expected: existing tests still pass.

- [ ] **Step 3: Commit** `feat(db): migration for post_views, posts, events`.

---

### Task A2: Domain wire types

**Files:**
- Modify: `crates/chaos-domain/src/dashboard.rs` (add types + re-exported via existing `pub use dashboard::*`)
- Test: same file `mod tests`

- [ ] **Step 1: Write failing tests.**

```rust
#[test]
fn view_event_serde_snake_case() {
    assert_eq!(serde_json::to_string(&ViewEvent::OpenedComments).unwrap(), "\"opened_comments\"");
    assert_eq!(serde_json::from_str::<ViewEvent>("\"seen\"").unwrap(), ViewEvent::Seen);
}
#[test]
fn view_flags_from_times() {
    assert_eq!(ViewFlags::from_times(true, false, false),
               ViewFlags { seen: true, comments: false, article: false });
    // opening implies seen at the render layer, but from_times reflects the columns verbatim.
    assert_eq!(ViewFlags::from_times(false, false, false), ViewFlags::default());
}
```

- [ ] **Step 2: Run, verify fail.** Run: `cargo test -p chaos-domain view_event view_flags -v`. Expected: FAIL (types missing).

- [ ] **Step 3: Add the types** (place near `Source`/`FeedItem` in `dashboard.rs`; they re-export through `lib.rs`'s `pub use dashboard::*`):

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewEvent {
    Seen,
    OpenedComments,
    OpenedArticle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ViewFlags {
    pub seen: bool,
    pub comments: bool,
    pub article: bool,
}
impl ViewFlags {
    pub fn from_times(seen: bool, comments: bool, article: bool) -> Self {
        Self { seen, comments, article }
    }
}

pub type ViewedMap = std::collections::HashMap<String, ViewFlags>;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ViewEventItem {
    pub source: Source,
    pub post_id: String,
    pub event: ViewEvent,
    pub at: chrono::DateTime<chrono::Utc>,
}
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct RecordViewsRequest {
    pub events: Vec<ViewEventItem>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EventItem {
    pub kind: String,
    pub detail: Option<String>,
    pub at: chrono::DateTime<chrono::Utc>,
}
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct RecordEventsRequest {
    pub events: Vec<EventItem>,
}
```

- [ ] **Step 4: Run tests + `cargo build --workspace`.** Expected: green.

- [ ] **Step 5: Commit** `feat(domain): ViewEvent/ViewFlags/ViewedMap + record requests`.

---

### Task A3: `db_views.rs`

**Files:**
- Create: `crates/chaos-server/src/db_views.rs`
- Modify: `crates/chaos-server/src/db.rs` (add `mod db_views;` — check how `db_calendar`/`db_auth` are declared; they're `mod` in `db.rs` or `main.rs`, match it)
- Test: `crates/chaos-server/src/db_views.rs` `mod tests`

- [ ] **Step 1: Write failing tests** (mirror `db_calendar.rs` tests: they build an in-memory `Db` and a user). Use the crate's existing test helper for a test `Db` + a created user (grep `db_calendar.rs`/`db_auth.rs` tests for `Db::connect`/`add_user`/the in-memory setup and reuse it verbatim).

```rust
#[tokio::test]
async fn record_view_is_first_write_wins_and_implies_seen() {
    let db = test_db().await;                 // reuse the crate's in-memory test-db helper
    let uid = test_user(&db, "tibo").await;   // reuse the crate's user-insert helper
    let t1 = Utc::now();
    db.record_view(uid, "hackernews", "1", ViewEvent::Seen, t1).await.unwrap();
    // second Seen must NOT move seen_at
    db.record_view(uid, "hackernews", "1", ViewEvent::Seen, t1 + Duration::hours(1)).await.unwrap();
    // OpenedComments sets comments + keeps seen; not article
    db.record_view(uid, "hackernews", "1", ViewEvent::OpenedComments, t1 + Duration::hours(2)).await.unwrap();

    let map = db.viewed_map(uid, "hackernews").await.unwrap();
    let f = map.get("1").copied().unwrap();
    assert_eq!(f, ViewFlags { seen: true, comments: true, article: false });
}

#[tokio::test]
async fn viewed_map_is_user_scoped() {
    let db = test_db().await;
    let a = test_user(&db, "a").await;
    let b = test_user(&db, "b").await;
    db.record_view(a, "lobsters", "x", ViewEvent::Seen, Utc::now()).await.unwrap();
    assert!(db.viewed_map(b, "lobsters").await.unwrap().is_empty());
    assert_eq!(db.viewed_map(a, "lobsters").await.unwrap().len(), 1);
}
```

> If the crate has no reusable `test_db`/`test_user` helpers, copy the exact
> setup used in `db_calendar.rs`'s `#[tokio::test]` fixtures (in-memory
> `sqlite::memory:` pool + `add_user`). Do NOT invent a new pattern.

- [ ] **Step 2: Run, verify fail.** Run: `cargo test -p chaos-server record_view viewed_map -v`. Expected: FAIL.

- [ ] **Step 3: Implement `db_views.rs`:**

```rust
use chaos_domain::{Source, ViewEvent, ViewFlags, ViewedMap};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::db::Db;

impl Db {
    /// Record one engagement event, first-timestamp-wins. Every event ensures
    /// `seen_at` is set; opened_* set their own column when null.
    pub async fn record_view(
        &self,
        user_id: Uuid,
        source: &str,
        post_id: &str,
        event: ViewEvent,
        at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        // Columns to set-if-null for this event.
        let (set_comments, set_article) = match event {
            ViewEvent::Seen => (false, false),
            ViewEvent::OpenedComments => (true, false),
            ViewEvent::OpenedArticle => (false, true),
        };
        sqlx::query(
            "INSERT INTO post_views
                (user_id, source, post_id, seen_at, opened_comments_at, opened_article_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?4)
             ON CONFLICT(user_id, source, post_id) DO UPDATE SET
                seen_at            = COALESCE(post_views.seen_at, excluded.seen_at),
                opened_comments_at = COALESCE(post_views.opened_comments_at, excluded.opened_comments_at),
                opened_article_at  = COALESCE(post_views.opened_article_at, excluded.opened_article_at),
                updated_at         = excluded.updated_at",
        )
        .bind(user_id.to_string())
        .bind(source)
        .bind(post_id)
        .bind(at)                                   // seen_at (+updated_at via ?4)
        .bind(set_comments.then_some(at))           // opened_comments_at or NULL
        .bind(set_article.then_some(at))            // opened_article_at or NULL
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn viewed_map(&self, user_id: Uuid, source: &str) -> Result<ViewedMap, sqlx::Error> {
        let rows: Vec<(String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT post_id, seen_at, opened_comments_at, opened_article_at
             FROM post_views WHERE user_id = ?1 AND source = ?2",
        )
        .bind(user_id.to_string())
        .bind(source)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, s, c, a)| {
                (id, ViewFlags::from_times(s.is_some(), c.is_some(), a.is_some()))
            })
            .collect())
    }
}
```

> `self.pool` is `pub(crate)` (confirmed in the map). `.bind(at)` binds a
> `DateTime<Utc>` directly (sqlx maps it to RFC3339 TEXT, per `db.rs`
> conventions). If sqlx complains about binding `DateTime` on this SQLite
> setup, bind `at.to_rfc3339()` instead — check how `db_calendar.rs` binds
> timestamps and match it exactly.

- [ ] **Step 4: Run tests + clippy.** Expected: green.

- [ ] **Step 5: Commit** `feat(db): per-user post view upsert + viewed_map`.

---

### Task A4: `db_posts.rs` + `db_events.rs`

**Files:**
- Create: `crates/chaos-server/src/db_posts.rs`, `crates/chaos-server/src/db_events.rs`
- Modify: `crates/chaos-server/src/db.rs` (`mod db_posts; mod db_events;` matching A3's mod-declaration location)

- [ ] **Step 1: Write failing tests.**

```rust
// db_posts.rs
#[tokio::test]
async fn upsert_posts_keeps_first_seen() {
    let db = test_db().await;
    let t1 = Utc::now();
    db.upsert_posts(&[("hackernews".into(), "1".into(), "Title".into())], t1).await.unwrap();
    db.upsert_posts(&[("hackernews".into(), "1".into(), "Title changed".into())], t1 + Duration::hours(1)).await.unwrap();
    let first = db.post_first_seen("hackernews", "1").await.unwrap();  // test-only read helper
    assert_eq!(first, Some(t1.timestamp()));  // compare at second granularity via a helper
}

// db_events.rs
#[tokio::test]
async fn record_events_inserts_rows() {
    let db = test_db().await;
    let uid = test_user(&db, "tibo").await;
    db.record_events(Some(uid), &[
        EventItem { kind: "app_open".into(), detail: None, at: Utc::now() },
        EventItem { kind: "reader_open".into(), detail: Some("hackernews:1".into()), at: Utc::now() },
    ]).await.unwrap();
    assert_eq!(db.count_events("app_open").await.unwrap(), 1);  // test-only count helper
}
```

- [ ] **Step 2: Run, verify fail.** Run: `cargo test -p chaos-server upsert_posts record_events -v`. Expected: FAIL.

- [ ] **Step 3: Implement.**

`db_posts.rs`:
```rust
use chrono::{DateTime, Utc};
use crate::db::Db;

impl Db {
    /// Record first-seen for each (source, post_id); existing rows are left
    /// untouched (first_seen_at is the earliest sighting).
    pub async fn upsert_posts(
        &self,
        items: &[(String, String, String)],   // (source, post_id, title)
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        for (source, post_id, title) in items {
            sqlx::query(
                "INSERT INTO posts (source, post_id, title, first_seen_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(source, post_id) DO NOTHING",
            )
            .bind(source)
            .bind(post_id)
            .bind(title)
            .bind(now)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }
}
```

`db_events.rs`:
```rust
use chaos_domain::EventItem;
use uuid::Uuid;
use crate::db::Db;

impl Db {
    pub async fn record_events(
        &self,
        user_id: Option<Uuid>,
        items: &[EventItem],
    ) -> Result<(), sqlx::Error> {
        let uid = user_id.map(|u| u.to_string());
        for it in items {
            sqlx::query(
                "INSERT INTO events (id, user_id, kind, at, detail) VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .bind(Uuid::now_v7().to_string())
            .bind(uid.as_deref())
            .bind(&it.kind)
            .bind(it.at)
            .bind(it.detail.as_deref())
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// Convenience for server-side single events (login/search).
    pub async fn record_event(
        &self,
        user_id: Option<Uuid>,
        kind: &str,
        at: chrono::DateTime<chrono::Utc>,
        detail: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        self.record_events(
            user_id,
            &[EventItem { kind: kind.into(), detail: detail.map(str::to_owned), at }],
        )
        .await
    }
}
```

Add the small `#[cfg(test)]` read helpers used by the tests (`post_first_seen`,
`count_events`) in the respective files' test modules or as `#[cfg(test)]`
methods.

- [ ] **Step 4: Run tests + clippy + fmt.** Expected: green.

- [ ] **Step 5: Commit** `feat(db): posts ingestion upsert + events log`.

---

### Task A5: Endpoints

**Files:**
- Modify: `crates/chaos-server/src/api/widgets.rs` (add `post_views_map`, `record_post_views`, `record_events`, and posts upsert in `posts_list`) OR a new `api/views.rs` module wired in `api/mod.rs` — follow whichever matches the codebase (calendar has its own `api/calendar.rs`; prefer a new `api/views.rs`).
- Modify: `crates/chaos-server/src/api/mod.rs` (routes)

- [ ] **Step 1: Add handlers** (mirror `api/calendar.rs` `AuthUser` handlers):

```rust
use chaos_domain::{RecordEventsRequest, RecordViewsRequest, Source, ViewedMap};

pub async fn views_map(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Path(source): Path<String>,
) -> Result<Json<ViewedMap>, ApiError> {
    let src = Source::from_str(&source).ok_or(ApiError::NotFound)?;
    Ok(Json(state.db.viewed_map(user.id, src.as_str()).await?))
}

pub async fn record_views(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Json(req): Json<RecordViewsRequest>,
) -> Result<StatusCode, ApiError> {
    for e in &req.events {
        state.db.record_view(user.id, e.source.as_str(), &e.post_id, e.event, e.at).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn record_events(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Json(req): Json<RecordEventsRequest>,
) -> Result<StatusCode, ApiError> {
    state.db.record_events(Some(user.id), &req.events).await?;
    Ok(StatusCode::NO_CONTENT)
}
```

> Match `ApiError`'s `From<sqlx::Error>` (used by calendar handlers via `?`) and
> the `StatusCode`/`Json` imports already used in the module. If handlers there
> return `Result<(), ApiError>` for 204, mirror that instead of `StatusCode`.

- [ ] **Step 2: Add routes** in `api/mod.rs` next to the posts routes:

```rust
.route("/posts/{source}/views", get(views::views_map))
.route("/posts/views", post(views::record_views))
.route("/events", post(views::record_events))
```

- [ ] **Step 3: Posts ingestion in `posts_list`.** In the existing `posts_list`
handler, after obtaining the `WidgetData::Posts(posts)`, collect items and
upsert (only for this handler):

```rust
if let WidgetData::Posts(p) = &data {
    let items: Vec<(String, String, String)> = p.last_24h.iter()
        .chain(&p.last_48h).chain(&p.last_week)
        .filter_map(|i| i.id.clone().map(|id| (source.as_str().to_string(), id, i.title.clone())))
        .collect();
    let _ = state.db.upsert_posts(&items, chrono::Utc::now()).await; // best-effort
}
```

- [ ] **Step 4: Tests.** Add handler tests mirroring calendar's: `views_map`
unknown source → 404; the three routes reject unauthenticated requests → 401
(build the app and call without a token, as existing auth-gated tests do). Run
`cargo test -p chaos-server`.

- [ ] **Step 5: Commit** `feat(server): views/events endpoints + posts ingestion`.

---

### Task A6: Server-side login + search logging

**Files:**
- Modify: `crates/chaos-server/src/api/auth.rs` (login handler)
- Modify: `crates/chaos-server/src/api/search.rs` (search handler)

- [ ] **Step 1: Log `login`.** In the login handler, right after
`create_session(...).await?` succeeds, record the event with the request's
user-agent:

```rust
let ua = headers.get(axum::http::header::USER_AGENT).and_then(|v| v.to_str().ok());
let _ = state.db.record_event(Some(user.id), "login", Utc::now(), ua).await; // best-effort
```

- [ ] **Step 2: Log `search`.** In the search handler, after resolving the
optional user, record the query:

```rust
let uid = crate::auth::optional_user_id(&state, &headers).await; // match the real signature
let _ = state.db.record_event(uid, "search", Utc::now(), Some(query.q.as_str())).await;
```

> Use the real `optional_user_id` signature/param names (the map notes
> `optional_user_id(state, headers) -> Option<Uuid>`); and the real query field
> name on `SearchQuery` (grep it — likely `q`).

- [ ] **Step 3: Test.** Add/extend a server test: a successful login inserts one
`login` event (use the `count_events` test helper from A4). Run
`cargo test -p chaos-server`.

- [ ] **Step 4: Commit** `feat(server): log login and search events`.

---

### Task A7: Client typed calls

**Files:**
- Modify: `crates/chaos-client/src/lib.rs`

- [ ] **Step 1: Add calls** next to `posts_list`/`post_thread`, using the
existing `get` and `send_no_content` helpers (grep an existing POST-204 call —
e.g. around lines 186-311 — to copy the `self.http.post(self.url(path)?).json(body)` + `send_no_content` shape):

```rust
pub async fn viewed_map(&self, source: chaos_domain::Source) -> Result<chaos_domain::ViewedMap> {
    self.get(&format!("api/v1/posts/{}/views", source.as_str())).await
}
pub async fn record_views(&self, req: &chaos_domain::RecordViewsRequest) -> Result<()> {
    let req = self.http.post(self.url("api/v1/posts/views")?).json(req);
    self.send_no_content(req).await
}
pub async fn record_events(&self, req: &chaos_domain::RecordEventsRequest) -> Result<()> {
    let req = self.http.post(self.url("api/v1/events")?).json(req);
    self.send_no_content(req).await
}
```

- [ ] **Step 2: Test** (URL building, mirror the existing client URL tests if
present): assert `viewed_map` targets `api/v1/posts/hackernews/views`. Run
`cargo test -p chaos-client`.

- [ ] **Step 3: Full workspace check.** `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check`. Expected: green.

- [ ] **Step 4: Commit** `feat(client): viewed_map + record_views/record_events`.

---

## Self-review notes
- Spec coverage: A1 = tables; A2 = domain types; A3 = post_views upsert/read;
  A4 = posts + events DB; A5 = endpoints + ingestion; A6 = login/search logging;
  A7 = client calls. All §Plan A spec items covered.
- Type consistency: `ViewEvent`/`ViewFlags`/`ViewedMap`/`ViewEventItem`/
  `RecordViewsRequest`/`EventItem`/`RecordEventsRequest` defined in A2 and used
  verbatim in A3/A5/A7; `record_view(user_id, source:&str, post_id:&str, event, at)`,
  `viewed_map(user_id, source:&str)`, `upsert_posts(&[(String,String,String)], now)`,
  `record_events(Option<Uuid>, &[EventItem])`, `record_event(Option<Uuid>, &str, at, Option<&str>)`
  consistent across tasks.
- Auth: all three new endpoints use `AuthUser` (401 unauth). Ingestion + login +
  search logging are best-effort (`let _ =`) so analytics never breaks a request.
- No pruning (spec: keep history).
