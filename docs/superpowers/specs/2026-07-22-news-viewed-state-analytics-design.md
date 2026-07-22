# News Viewed-State + Engagement Analytics Design

**Date:** 2026-07-22
**Status:** Approved (design). Implementation split into two plans (A backend/data, B client/UI).

## Problem

Authenticated users (web + Android) want to see, at a glance, which HN/lobsters
posts they've already engaged with, distinguished by *how*: never touched, just
seen in the list, opened the comments, opened the article link, or both — synced
across devices. Alongside, the owner wants durable engagement analytics
(timestamps of first interactions, connection/usage events) to mine later.

## Decisions (from brainstorming, 2026-07-22)

- **Three underlying per-post signals** (booleans backed by first-occurrence
  timestamps): `seen`, `opened_comments`, `opened_article`.
- **Visual rule:** dimming is the *reading* axis only, the check is the article
  axis, and they are independent:
  - **Title opacity** = `opened_comments ? ~0.52 : (seen && !opened_article ? ~0.72 : 1.0)`.
    Opening the article suppresses the seen-dim, so an article-only row stays
    full brightness.
  - **Article check** (accent `✓`, placed **between the domain and the favicon**)
    shown iff `opened_article`.
  - Five states: never-seen (bright, no check) · seen (light dim) ·
    opened-comments (dim) · opened-article (bright + check) · both (dim + check).
- **"Seen" trigger:** the row scrolling into the viewport (IntersectionObserver).
- **Offline:** an outbox (new infrastructure) queues events in localStorage and
  flushes on the health-probe Offline→Online transition.
- **Analytics events logged** (generic `events` table): `login` (server),
  `app_open` (client, ≤ once / 5 min / device, skipped if logged recently),
  `search` (server), `reader_open` (client, every visit).
- **Post ingestion timestamps:** a `posts` table records `first_seen_at` per
  `(source, post_id)` so analytics can compare "added to DB" vs read/comments.
- **Retention:** nothing is pruned — this is analytics data the owner wants to
  keep. Tables grow slowly.
- **Scope:** the `/news` page + reader, authenticated only. The desktop
  dashboard widget is unchanged (renders plain rows). Logged-off = plain rows,
  no tracking.

## Build order

- **Plan A — backend/data:** migration (3 tables), domain wire types, DB
  modules, server endpoints, server-side `login`/`search` logging, post
  ingestion, typed client calls.
- **Plan B — client/UI:** analytics module (outbox + optimistic overlay + flush
  + reconnect + `app_open` throttle), NewsPage load + IntersectionObserver +
  context, `post_row_view` rendering + record hooks, reader recording, CSS.

---

## Plan A — backend & data

### A1. Migration `0005_views_analytics.sql`

Three tables (conventions per `db.rs`: UUID = hyphenated TEXT, timestamps =
RFC3339 TEXT, FKs to `users(id)`).

```sql
-- Per-user, per-post engagement. First-occurrence timestamps; a set column
-- means that signal is on. Opening always also sets seen_at.
CREATE TABLE post_views (
    user_id            TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    source             TEXT NOT NULL,          -- "hackernews" | "lobsters"
    post_id            TEXT NOT NULL,          -- provider id
    seen_at            TEXT,
    opened_comments_at TEXT,
    opened_article_at  TEXT,
    updated_at         TEXT NOT NULL,
    PRIMARY KEY (user_id, source, post_id)
);
CREATE INDEX idx_post_views_user ON post_views(user_id);

-- System-wide post ingestion: when a (source, post_id) first entered our DB.
CREATE TABLE posts (
    source        TEXT NOT NULL,
    post_id       TEXT NOT NULL,
    title         TEXT NOT NULL,
    first_seen_at TEXT NOT NULL,
    PRIMARY KEY (source, post_id)
);

-- Generic append-only analytics log.
CREATE TABLE events (
    id      TEXT PRIMARY KEY,
    user_id TEXT REFERENCES users(id) ON DELETE SET NULL,  -- NULL = anonymous
    kind    TEXT NOT NULL,      -- login | app_open | search | reader_open | …
    at      TEXT NOT NULL,
    detail  TEXT               -- free-form: user-agent, query, "source:post_id"
);
CREATE INDEX idx_events_kind_at ON events(kind, at);
CREATE INDEX idx_events_user ON events(user_id, at);
```

### A2. Domain wire types (`chaos-domain/src/dashboard.rs` or new `views.rs`)

```rust
/// A per-post interaction event the client records.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewEvent { Seen, OpenedComments, OpenedArticle }

/// The three booleans rendered per row (derived from the *_at columns).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewFlags { pub seen: bool, pub comments: bool, pub article: bool }

/// Server → client: the current user's flags for a source, keyed by post_id.
pub type ViewedMap = std::collections::HashMap<String, ViewFlags>;

/// One queued post-view event (carries the client timestamp for offline).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewEventItem {
    pub source: Source,
    pub post_id: String,
    pub event: ViewEvent,
    pub at: DateTime<Utc>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RecordViewsRequest { pub events: Vec<ViewEventItem> }

/// One queued generic analytics event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventItem { pub kind: String, pub detail: Option<String>, pub at: DateTime<Utc> }
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RecordEventsRequest { pub events: Vec<EventItem> }
```

`ViewFlags` derivation helper (pure, unit-tested): `ViewFlags::from_times(seen, comments, article: (bool,bool,bool))`.

### A3. DB modules

- **`db_views.rs`** — `impl Db`:
  - `record_view(user_id, source, post_id, event, at)`: upsert. `seen_at`
    COALESCE-set (first-wins) on every event; `opened_comments_at` set-if-null
    on `OpenedComments`; `opened_article_at` set-if-null on `OpenedArticle`;
    `updated_at = now`. Use `INSERT … ON CONFLICT(user_id,source,post_id) DO UPDATE SET …`
    with `COALESCE(column, excluded.column)` so first timestamp wins.
  - `viewed_map(user_id, source) -> ViewedMap`: select post_id + the three
    `*_at IS NOT NULL` booleans for that user+source.
  - User-scoped: every query filters `user_id = ?` (another user's rows are
    invisible), mirroring `db_calendar.rs`.
- **`db_posts.rs`** — `impl Db`: `upsert_posts(items: &[(source, post_id, title)])`
  — `INSERT … ON CONFLICT(source,post_id) DO NOTHING` (first_seen_at stays the
  first). One statement per item or a batch.
- **`db_events.rs`** — `impl Db`:
  - `record_event(user_id: Option<Uuid>, kind, at, detail)` — INSERT (id = v7).
  - `record_events(user_id, items: &[EventItem])` — batch insert.

### A4. Server endpoints (`api/`)

New handlers (mirror `api/calendar.rs` shape; routes in `api/mod.rs`):

- `GET /api/v1/posts/{source}/views` — `AuthUser` → `Json<ViewedMap>`.
  `Source::from_str` or 404.
- `POST /api/v1/posts/views` — `AuthUser`, `Json<RecordViewsRequest>` → 204.
  Loops `db.record_view(user.id, …)` per item.
- `POST /api/v1/events` — `AuthUser`, `Json<RecordEventsRequest>` → 204.
  `db.record_events(Some(user.id), &req.events)`.

Server-side event logging (no client involvement):

- **`login`** — in `api/auth.rs` login handler, after the session is created:
  `db.record_event(Some(user.id), "login", now, user_agent_from_headers)`.
- **`search`** — in `api/search.rs`, log `db.record_event(optional_user_id, "search", now, Some(query))`.

Post ingestion:

- In the `posts_list` handler (`api/widgets.rs`), after obtaining the
  `WidgetData::Posts`, extract every `(source, id, title)` from the three
  windows and `db.upsert_posts(...)`. Idempotent; runs on every response
  (cache hit or miss). Only the `/posts/{source}` path needs this.

### A5. Client typed calls (`chaos-client/src/lib.rs`)

```rust
pub async fn viewed_map(&self, source: Source) -> Result<ViewedMap> {
    self.get(&format!("api/v1/posts/{}/views", source.as_str())).await
}
pub async fn record_views(&self, req: &RecordViewsRequest) -> Result<()> {
    self.post_no_content("api/v1/posts/views", req).await  // match the crate's POST-204 helper
}
pub async fn record_events(&self, req: &RecordEventsRequest) -> Result<()> {
    self.post_no_content("api/v1/events", req).await
}
```

(Use the crate's existing send-no-content POST helper; check `send_no_content`.)

### A6. Tests (Plan A)

- `db_views`: `record_view` idempotency (second `Seen` keeps first `seen_at`;
  `OpenedComments` sets comments + seen but not article); per-user isolation
  (user B's `viewed_map` excludes user A's rows); `viewed_map` booleans.
- `db_posts`: `upsert_posts` sets `first_seen_at` once (re-upsert is a no-op).
- `db_events`: `record_events` inserts N rows with the given `kind`/`at`.
- Handlers: `GET …/views` unknown source → 404; `AuthUser` gating → 401 when
  unauthenticated (mirror existing calendar handler tests).
- `ViewFlags::from_times` derivation.

---

## Plan B — client & UI

### B1. Analytics module (`chaos-ui/src/analytics.rs`, new)

The offline-capable outbox + optimistic overlay. Global (used by NewsPage and
reader).

- **State:**
  - `OUTBOX_KEY = "chaos-view-outbox"` (view events) and
    `EVENTS_KEY = "chaos-event-outbox"` (generic events) in localStorage,
    JSON arrays.
  - `Overlay = RwSignal<HashMap<(Source, String), ViewFlags>>` provided once in
    `App` context (so NewsPage rows and the reader share it). Rows read their
    flags reactively.
- **`record_view(source, id, event)`**: OR the flag into the overlay
  immediately (instant restyle); append a `ViewEventItem { at: now }` to the
  view outbox (skip if that flag is already set in the overlay AND already
  synced — dedup to keep the outbox small); schedule a flush.
- **`record_event(kind, detail)`**: append an `EventItem` to the event outbox;
  schedule a flush.
- **`load_views(source, client)`**: `client.viewed_map(source)` → merge into the
  overlay (OR flags in; never clear locally-pending ones).
- **Flush** (`flush(conn, client)`): if `conn == Online`, POST the view outbox
  via `record_views` and the event outbox via `record_events`; on success clear
  the flushed items. Debounced (~1.5s) after a `record_*`; also called on the
  probe Offline→Online transition (hook in `offline::probe`). Offline → no-op
  (items persist).
- **`maybe_record_app_open()`**: read `chaos-appopen-at` from localStorage; if
  absent or > 5 min old, `record_event("app_open", None)` and rewrite the
  timestamp; else skip. Called once at App boot when a session exists.
- **Pure, unit-tested helpers:** outbox append/dedup, `merge_flags`, the
  5-min throttle predicate `should_record_app_open(last, now)`.

### B2. `ViewedState` context + NewsPage wiring (`pages/news.rs`)

- Provide a `ViewedState { source: Source, overlay: RwSignal<…> }` context from
  NewsPage **only when authenticated** (`use_session().0.get().is_some()`).
  Absent otherwise → `post_row_view` renders plain.
- On source load (and when authed), call `analytics::load_views(source, client)`.
- **IntersectionObserver**: after the list renders, observe each
  `li.post-row[data-view-id]` (data attr = `"{source}:{id}"`). On an entry with
  `intersectionRatio >= 0.5`, `analytics::record_view(source, id, Seen)` once.
  Re-observe when the list changes (source/range switch): an `Effect` keyed on
  the shown list disconnects + re-observes. Browser-only glue.

### B3. `post_row_view` rendering + record hooks (`pages/dashboard.rs`)

- Read `use_context::<ViewedState>()`. If present, compute this post's flags from
  `overlay.get()[(source, id)]`; else `ViewFlags::default()` (all false → plain).
- Add `data-view-id="{source}:{id}"` to the `<li>` (for the observer) when
  ViewedState is present and `id` is Some.
- Title opacity via a derived class: `post-row` gets `seen` / `read` classes:
  - `read` (opened_comments) → strongest dim;
  - `seen` (seen && !article && !comments) → light dim;
  - article-opened suppresses the seen-dim (no class) — handled by the class
    logic, not just CSS.
- **Article check**: render `<span class="article-check">"✓"</span>` inside
  `.post-meta-right`, before the favicon, when `flags.article`.
- **Record hooks** (only when ViewedState present):
  - title `<A>` (reader link) `on:click` → `record_view(source, id, OpenedComments)`.
  - favicon `<a>` `on:click` → `record_view(source, id, OpenedArticle)`.
  - (Seen is handled by the observer, not here.)

### B4. Reader wiring (`pages/reader.rs`)

- On successful thread load (authed): `record_view(source, id, OpenedComments)`
  (covers deep-links / direct navigation) and `record_event("reader_open", Some(format!("{source}:{id}")))`.
- The reader's story-title link to the article → `record_view(source, id, OpenedArticle)` on click.

### B5. App boot (`lib.rs`)

- After the session is confirmed (the `me()` success path / cached-user restore),
  call `analytics::maybe_record_app_open()`.
- Wire the reconnect flush: in `offline::probe`, on Offline→Online, call
  `analytics::flush(...)` (or expose a hook the App subscribes to).

### B6. CSS (`chaos-web/styles.css`)

```css
li.post-row.seen .post-title { opacity: 0.72; }
li.post-row.read .post-title { opacity: 0.52; }
.article-check { color: var(--accent); opacity: 0.85; font-size: 0.8rem; font-weight: 700; }
```

(`.article-check` sits inside `.post-meta-right`, before `.post-favicon`, so the
existing right-cluster flex places it between the domain and the icon.)

### B7. Tests (Plan B)

- Pure: `should_record_app_open(last, now)` (none/absent → true; < 5 min → false;
  ≥ 5 min → true); outbox append + dedup; `merge_flags` OR semantics; row-class
  derivation from `ViewFlags` (never/seen/read + article independence).
- Browser (headless, verified before ship): scroll marks rows seen (dim);
  tapping a title dims + (on return) stays; tapping a favicon adds the check
  without dimming; offline interactions flush on reconnect (best-effort check).

## Security / auth

- All three endpoints require `AuthUser` (401 otherwise). Views/events are
  strictly user-scoped in the DB; another user's data is never returned.
- `search` logging uses `optional_user_id` (may be anonymous); everything else
  is authenticated.
- No new external egress; no untrusted HTML. `detail` is stored verbatim and
  only ever read by the owner's analytics (not rendered into the UI).

## Non-goals (YAGNI)

- No analytics *dashboard/UI* — this stores the data; querying it is manual
  (SQL) for now.
- No pruning/retention limits (owner wants the history).
- No desktop-widget viewed-state (clean seam left; can adopt later).
- No per-comment read tracking; `reader_open` dwell is a single timestamp, not
  duration.
