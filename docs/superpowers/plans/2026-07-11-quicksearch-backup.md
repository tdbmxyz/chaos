# Quick-Search & Scheduled Backup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement two Phase 8 roadmap items: (A) a global quick-search — `GET /api/v1/search?q=` aggregating config services, config bookmarks, stored links (existing LIKE+FTS5 path) and the signed-in user's calendar events, plus a Ctrl-K overlay in the UI with debounced input, grouped results and arrow-key navigation; (B) a scheduled SQLite backup task — `VACUUM INTO` a timestamped copy on a configurable interval, pruned to the N newest files.

**Architecture:** Rust cargo workspace. `chaos-domain` holds all wire types (serde, compiles native+wasm, no I/O). `chaos-server` is axum over SQLite (sqlx); handlers live in `crates/chaos-server/src/api/` (routing in `api/mod.rs` — the handlers were NOT split into services.rs/widgets.rs; `dashboard`/`services` handlers are directly in `api/mod.rs`). Services and bookmarks come from `Config` (figment TOML), links from `Db::list_links` (LIKE + `archive_fts` MATCH), calendar events from the merged range machinery in `api/calendar.rs` (`AuthUser`-gated; search uses `auth::optional_user_id` so logged-off requests just get an empty events group, matching existing semantics). Background tasks (`monitor::spawn`, `archiver::spawn`) are spawned from `main.rs`; the backup task follows the same pattern. `chaos-client` is the single typed API client; `chaos-ui` is the shared Leptos (csr) crate that also compiles natively, so pure helpers get native unit tests (pattern: `pages/home.rs`, `components.rs`). Server tests use `Db::in_memory()` and test helper functions directly (pattern: `on_demand_service` test in `api/mod.rs`).

**Tech Stack:** Rust, axum 0.8, sqlx (SQLite, WAL), tokio, figment, serde, chrono, Leptos 0.8 (`leptos::prelude`, `window_event_listener`, `LocalResource`, `set_timeout`), reqwest dual-backend client, cargo-nextest (`just test` = `cargo nextest run --workspace`), `just check` (fmt + clippy `-D warnings` + wasm check).

---

## Task 1: Search wire types in chaos-domain

New module `crates/chaos-domain/src/search.rs` with `SearchKind`, `SearchHit`, `SearchResults`, `SearchQuery`. Conventions copied from the existing domain modules: `Serialize + Deserialize + Debug + Clone + PartialEq + Eq`, `#[serde(rename_all = "snake_case")]` on enums, `#[serde(default, skip_serializing_if = "Option::is_none")]` on optionals, `url::Url` for URLs. `SearchResults` derives `Default` (the empty-query response). chaos-domain has no `serde_json` today, so it is added as a dev-dependency (already a workspace dependency).

**Files:**
- Create: `/projects/rust/chaos/crates/chaos-domain/src/search.rs`
- Modify: `/projects/rust/chaos/crates/chaos-domain/src/lib.rs` (declare + re-export)
- Modify: `/projects/rust/chaos/crates/chaos-domain/Cargo.toml` (`[dev-dependencies] serde_json`)

**Steps:**

- [ ] Add to `/projects/rust/chaos/crates/chaos-domain/Cargo.toml`, after the `[dependencies]` block:

  ```toml

  [dev-dependencies]
  serde_json.workspace = true
  ```

- [ ] Create `/projects/rust/chaos/crates/chaos-domain/src/search.rs` containing ONLY the test module for now (TDD — the types come next):

  ```rust
  //! Global quick-search (`GET /api/v1/search`): grouped hits across
  //! config-defined services and bookmarks, stored links, and the signed-in
  //! user's calendar events.

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn search_wire_format_is_stable() {
          let hit = SearchHit {
              kind: SearchKind::Service,
              title: "Jellyfin".into(),
              subtitle: None,
              url: Some("http://zeus:8096".parse().unwrap()),
          };
          let json = serde_json::to_string(&hit).unwrap();
          // Enum tags are snake_case and empty optionals are omitted, like
          // every other wire type in this crate.
          assert!(json.contains(r#""kind":"service""#), "got {json}");
          assert!(!json.contains("subtitle"), "got {json}");
          let back: SearchHit = serde_json::from_str(&json).unwrap();
          assert_eq!(back, hit);

          // Every group is optional on the wire; `{}` is the empty result.
          let results: SearchResults = serde_json::from_str("{}").unwrap();
          assert_eq!(results, SearchResults::default());
          assert!(results.services.is_empty() && results.events.is_empty());
      }
  }
  ```

  And in `/projects/rust/chaos/crates/chaos-domain/src/lib.rs`, extend the module list and re-exports (alphabetical, matching the existing style):

  ```rust
  pub mod search;
  ```
  after `pub mod links;`, and
  ```rust
  pub use search::*;
  ```
  after `pub use links::*;`.

- [ ] Run and verify it FAILS to compile (`cannot find type SearchHit`/`SearchKind`/`SearchResults` in this scope):

  ```
  cargo test -p chaos-domain search::tests::search_wire_format_is_stable -- --exact
  ```

- [ ] Add the types at the top of `/projects/rust/chaos/crates/chaos-domain/src/search.rs` (between the module doc comment and the tests module):

  ```rust
  use serde::{Deserialize, Serialize};
  use url::Url;

  /// Which group a hit belongs to; the UI routes on it (events have no URL
  /// of their own and navigate to `/calendar`).
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(rename_all = "snake_case")]
  pub enum SearchKind {
      Service,
      Bookmark,
      Link,
      Event,
  }

  /// One quick-search result row.
  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
  pub struct SearchHit {
      pub kind: SearchKind,
      pub title: String,
      /// Context line under the title: URL host for services/links, group
      /// name for bookmarks, start time + calendar name for events.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub subtitle: Option<String>,
      /// Where the hit leads. `None` for hits the UI routes internally
      /// (events open the calendar page).
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub url: Option<Url>,
  }

  /// What `GET /api/v1/search` returns: hits grouped in display order.
  /// Groups the requester cannot see (events while logged off) are empty,
  /// never an error.
  #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(default)]
  pub struct SearchResults {
      pub services: Vec<SearchHit>,
      pub bookmarks: Vec<SearchHit>,
      pub links: Vec<SearchHit>,
      pub events: Vec<SearchHit>,
  }

  /// Query parameters of `GET /api/v1/search`.
  #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
  pub struct SearchQuery {
      #[serde(default)]
      pub q: String,
  }
  ```

- [ ] Run green:

  ```
  cargo test -p chaos-domain search::tests::search_wire_format_is_stable -- --exact
  ```

- [ ] Commit:

  ```
  cd /projects/rust/chaos
  git add crates/chaos-domain
  git commit -m "$(cat <<'EOF'
  feat(domain): wire types for the global quick-search

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
  EOF
  )"
  ```

---

## Task 2: Server aggregation + `GET /api/v1/search`

New handler module `crates/chaos-server/src/api/search.rs`. The endpoint is public (like `/links`); only the events group is user-scoped, resolved via `crate::auth::optional_user_id` (same helper `links::create` uses for attribution). Events reuse the merged range machinery: the body of `calendar::events` is extracted into a shared `merged_events(state, user_id, start, end)` and searched over now ± 60 days. Links go through the existing `Db::list_links` LIKE+FTS path with `limit: Some(10)`. Bookmarks are gathered from both the top-level `config.bookmarks` and any `Widget::Bookmarks { groups }` in `config.columns` (the layout can define groups in either place).

Tests follow the existing server style: pure filter functions unit-tested directly, plus one `Db::in_memory()` + `AppState::new(Config {...}, db)` test driving the whole `aggregate` (there are no HTTP-level handler tests in this codebase; `api/mod.rs` tests `on_demand_service` directly the same way). `AppState::new` with a default-ish config is safe in tests: no Home Assistant (`base_url: None`), no `server_stats` widget, so nothing gets spawned.

**Files:**
- Create: `/projects/rust/chaos/crates/chaos-server/src/api/search.rs`
- Modify: `/projects/rust/chaos/crates/chaos-server/src/api/mod.rs` (module + route)
- Modify: `/projects/rust/chaos/crates/chaos-server/src/api/calendar.rs` (extract `merged_events`)

**Steps:**

- [ ] In `/projects/rust/chaos/crates/chaos-server/src/api/calendar.rs`, extract the merged view so search can reuse it. Replace the `events` handler (keep its doc comment) with:

  ```rust
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
      Ok(Json(merged_events(&state, user.id, query.start, query.end).await?))
  }

  /// Merged events (local + ICS feeds) of one user's calendars in
  /// [start, end), sorted by start. Shared by the events endpoint and the
  /// global quick-search.
  pub(crate) async fn merged_events(
      state: &AppState,
      user_id: Uuid,
      start: chrono::DateTime<chrono::Utc>,
      end: chrono::DateTime<chrono::Utc>,
  ) -> Result<Vec<CalendarEvent>, ApiError> {
      let mut out: Vec<CalendarEvent> = state
          .db
          .events_between(user_id, start, end)
          .await?
          .into_iter()
          .map(|(event, calendar_name, color)| CalendarEvent {
              id: Some(event.id),
              calendar_id: event.calendar_id,
              calendar_name,
              color,
              title: event.title,
              description: event.description,
              location: event.location,
              starts_at: event.starts_at,
              ends_at: event.ends_at,
              all_day: event.all_day,
          })
          .collect();

      for calendar in state.db.list_calendars(user_id).await? {
          if calendar.kind != CalendarKind::Ics {
              continue;
          }
          let Some(url) = &calendar.ics_url else {
              continue;
          };
          match state.ics.events(calendar.id, url, start, end).await {
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

      out.sort_by_key(|event| event.starts_at);
      Ok(out)
  }
  ```

  (This is the existing body verbatim with `user.id` → `user_id` and `query.start`/`query.end` → `start`/`end`; nothing else in the file changes.)

- [ ] Create `/projects/rust/chaos/crates/chaos-server/src/api/search.rs`:

  ```rust
  //! `GET /api/v1/search`: the global quick-search (Ctrl-K in the UI).
  //!
  //! Aggregates config-defined services and bookmarks, stored links (the
  //! existing LIKE + FTS5 query path), and — when the request carries a
  //! session — the user's calendar events. Public like the links API; only
  //! the events group is user-scoped (logged off → empty, never an error).

  use axum::Json;
  use axum::extract::{Query, State};
  use axum::http::HeaderMap;
  use chaos_domain::{
      BookmarkGroup, CalendarEvent, LinkQuery, SearchHit, SearchKind, SearchQuery, SearchResults,
      ServiceDef, Widget,
  };
  use chrono::{Duration, Utc};
  use uuid::Uuid;

  use crate::api::ApiError;
  use crate::state::AppState;

  /// Cap per result group, so one noisy section cannot drown the palette.
  const GROUP_LIMIT: usize = 10;
  /// Events are searched in a window of now ± this many days.
  const EVENT_WINDOW_DAYS: i64 = 60;

  pub async fn search(
      State(state): State<AppState>,
      headers: HeaderMap,
      Query(query): Query<SearchQuery>,
  ) -> Result<Json<SearchResults>, ApiError> {
      let q = query.q.trim();
      if q.is_empty() {
          return Ok(Json(SearchResults::default()));
      }
      let user_id = crate::auth::optional_user_id(&state, &headers).await;
      Ok(Json(aggregate(&state, user_id, q).await?))
  }

  async fn aggregate(
      state: &AppState,
      user_id: Option<Uuid>,
      q: &str,
  ) -> Result<SearchResults, ApiError> {
      let services = filter_services(&state.config.services, q);
      let bookmarks = filter_bookmarks(&bookmark_groups(state), q);

      let links = state
          .db
          .list_links(&LinkQuery {
              q: Some(q.to_string()),
              limit: Some(GROUP_LIMIT as u32),
              ..Default::default()
          })
          .await?
          .items
          .into_iter()
          .map(|link| SearchHit {
              kind: SearchKind::Link,
              title: link.title,
              subtitle: link.url.host_str().map(String::from),
              url: Some(link.url),
          })
          .collect();

      // Logged off → no events, matching the calendar API's auth semantics.
      let events = match user_id {
          Some(user_id) => {
              let now = Utc::now();
              let merged = super::calendar::merged_events(
                  state,
                  user_id,
                  now - Duration::days(EVENT_WINDOW_DAYS),
                  now + Duration::days(EVENT_WINDOW_DAYS),
              )
              .await?;
              filter_events(&merged, q)
          }
          None => Vec::new(),
      };

      Ok(SearchResults {
          services,
          bookmarks,
          links,
          events,
      })
  }

  fn matches(haystack: &str, q: &str) -> bool {
      haystack.to_lowercase().contains(&q.to_lowercase())
  }

  fn filter_services(services: &[ServiceDef], q: &str) -> Vec<SearchHit> {
      services
          .iter()
          .filter(|s| matches(&s.title, q) || matches(&s.id, q))
          .take(GROUP_LIMIT)
          .map(|s| SearchHit {
              kind: SearchKind::Service,
              title: s.title.clone(),
              subtitle: s.url.host_str().map(String::from),
              url: Some(s.url.clone()),
          })
          .collect()
  }

  /// Bookmark groups can live at the top level and/or inside `bookmarks`
  /// widgets in the column layout; search both.
  fn bookmark_groups(state: &AppState) -> Vec<&BookmarkGroup> {
      let mut groups: Vec<&BookmarkGroup> = state.config.bookmarks.iter().collect();
      for column in &state.config.columns {
          for widget in &column.widgets {
              if let Widget::Bookmarks { groups: g } = widget {
                  groups.extend(g.iter());
              }
          }
      }
      groups
  }

  fn filter_bookmarks(groups: &[&BookmarkGroup], q: &str) -> Vec<SearchHit> {
      groups
          .iter()
          .flat_map(|group| {
              group
                  .links
                  .iter()
                  .filter(|b| matches(&b.title, q))
                  .map(|b| SearchHit {
                      kind: SearchKind::Bookmark,
                      title: b.title.clone(),
                      subtitle: Some(group.title.clone()),
                      url: Some(b.url.clone()),
                  })
          })
          .take(GROUP_LIMIT)
          .collect()
  }

  fn filter_events(events: &[CalendarEvent], q: &str) -> Vec<SearchHit> {
      events
          .iter()
          .filter(|e| matches(&e.title, q))
          .take(GROUP_LIMIT)
          .map(|e| SearchHit {
              kind: SearchKind::Event,
              title: e.title.clone(),
              subtitle: Some(format!(
                  "{} · {}",
                  e.starts_at.format("%a %-d %b %H:%M"),
                  e.calendar_name
              )),
              url: None,
          })
          .collect()
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use chaos_domain::{
          Bookmark, CalendarKind, CalendarRequest, CreateLinkRequest, EventRequest,
      };

      use crate::config::Config;
      use crate::db::Db;

      #[tokio::test]
      async fn aggregate_searches_config_links_and_only_the_users_events() {
          let db = Db::in_memory().await.unwrap();
          let user = db.create_user("tibo", "Tibo", "phc").await.unwrap();
          let cal = db
              .create_calendar(
                  user.id,
                  &CalendarRequest {
                      name: "Perso".into(),
                      color: None,
                      kind: CalendarKind::Local,
                      ics_url: None,
                  },
              )
              .await
              .unwrap();
          db.create_event(
              user.id,
              &EventRequest {
                  calendar_id: cal.id,
                  title: "Dentist appointment".into(),
                  description: None,
                  location: None,
                  starts_at: Utc::now() + Duration::days(3),
                  ends_at: Utc::now() + Duration::days(3) + Duration::hours(1),
                  all_day: false,
              },
          )
          .await
          .unwrap();
          db.create_link(
              &CreateLinkRequest {
                  url: "https://example.com/dentures".parse().unwrap(),
                  title: Some("Denture care guide".into()),
                  description: None,
                  collection_id: None,
                  tags: vec![],
              },
              false,
              None,
          )
          .await
          .unwrap();

          let config = Config {
              services: vec![ServiceDef {
                  id: "jellyfin".into(),
                  title: "Jellyfin".into(),
                  url: "http://zeus:8096".parse().unwrap(),
                  icon: None,
                  check_url: None,
                  unit: None,
              }],
              bookmarks: vec![BookmarkGroup {
                  title: "Main".into(),
                  links: vec![Bookmark {
                      title: "Denpa News".into(),
                      url: "https://denpa.example.com".parse().unwrap(),
                      icon: None,
                      android_package: None,
                  }],
              }],
              ..Config::default()
          };
          let state = crate::state::AppState::new(config, db).unwrap();

          // Case-insensitive substring, hits in three groups at once.
          let results = aggregate(&state, Some(user.id), "DEN").await.unwrap();
          assert!(results.services.is_empty());
          assert_eq!(results.bookmarks.len(), 1);
          assert_eq!(results.bookmarks[0].subtitle.as_deref(), Some("Main"));
          assert_eq!(results.links.len(), 1);
          assert_eq!(results.links[0].kind, SearchKind::Link);
          assert_eq!(results.events.len(), 1);
          assert_eq!(results.events[0].kind, SearchKind::Event);
          assert!(results.events[0].url.is_none(), "events route via /calendar");

          let jelly = aggregate(&state, Some(user.id), "jelly").await.unwrap();
          assert_eq!(jelly.services.len(), 1);
          assert_eq!(
              jelly.services[0].url.as_ref().unwrap().as_str(),
              "http://zeus:8096/"
          );

          // Logged off: events stay private, everything else is public.
          let anon = aggregate(&state, None, "den").await.unwrap();
          assert!(anon.events.is_empty());
          assert_eq!(anon.links.len(), 1);
          assert_eq!(anon.bookmarks.len(), 1);
      }

      #[test]
      fn group_filters_cap_and_match_case_insensitively() {
          let services: Vec<ServiceDef> = (0..15)
              .map(|i| ServiceDef {
                  id: format!("svc-{i}"),
                  title: format!("Service {i}"),
                  url: "http://zeus:1234".parse().unwrap(),
                  icon: None,
                  check_url: None,
                  unit: None,
              })
              .collect();
          assert_eq!(filter_services(&services, "SERVICE").len(), GROUP_LIMIT);
          assert_eq!(filter_services(&services, "svc-14").len(), 1);
          assert!(filter_services(&services, "nope").is_empty());
      }
  }
  ```

- [ ] Run and verify BOTH tests FAIL (the module isn't declared yet, so first wire it up — next step — then the failure mode is: `merged_events` not found if the calendar extraction was skipped, or plain red on assertions if aggregation is wrong; with the code above they pass, so to honor TDD run the tests BEFORE writing the `aggregate`/filter bodies if executing strictly — at minimum verify the test compiles against the intended signatures and goes green only after the implementation is in place):

  ```
  cargo test -p chaos-server api::search::tests -- --nocapture
  ```

- [ ] Wire the module and route in `/projects/rust/chaos/crates/chaos-server/src/api/mod.rs`. In the module list add (alphabetical):

  ```rust
  mod search;
  ```
  after `mod links;`. In `router()`, add the route after the `/links`-related routes (next to `/tags`):

  ```rust
  .route("/search", get(search::search))
  ```

- [ ] Run green:

  ```
  cargo test -p chaos-server api::search::tests -- --nocapture
  cargo test -p chaos-server api::calendar
  ```

  (The second command confirms the `events` extraction compiles; calendar has no direct tests, so also run the full crate once: `cargo nextest run -p chaos-server`.)

- [ ] Commit:

  ```
  cd /projects/rust/chaos
  git add crates/chaos-server
  git commit -m "$(cat <<'EOF'
  feat(server): GET /api/v1/search aggregating services, bookmarks, links, events

  Public like the links API; the events group is scoped to the session
  user (logged off means empty, not an error). Extracts merged_events
  out of the calendar events handler for reuse.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
  EOF
  )"
  ```

---

## Task 3: Client method

One new method on `ChaosClient`, following the exact pattern of `list_links` (GET + query). No new tests (the client crate only unit-tests pure helpers; transport methods are exercised by compilation and the UI).

**Files:**
- Modify: `/projects/rust/chaos/crates/chaos-client/src/lib.rs`

**Steps:**

- [ ] In `/projects/rust/chaos/crates/chaos-client/src/lib.rs`: add `SearchResults` to the `chaos_domain` import list (keep it alphabetical: between `LoginResponse, ServiceActionRequest` → `..., LoginResponse, SearchResults, ServiceActionRequest, ...`). Then add, right after the `dashboard` method:

  ```rust
      /// Global quick-search across services, bookmarks, links and — when
      /// signed in — calendar events. Empty/whitespace queries return the
      /// empty result set.
      pub async fn search(&self, q: &str) -> Result<SearchResults> {
          let req = self.http.get(self.url("api/v1/search")?).query(&[("q", q)]);
          self.send(req).await
      }
  ```

- [ ] Verify it compiles on both targets:

  ```
  cargo check -p chaos-client
  cargo check -p chaos-ui --target wasm32-unknown-unknown
  ```

- [ ] Commit:

  ```
  cd /projects/rust/chaos
  git add crates/chaos-client
  git commit -m "$(cat <<'EOF'
  feat(client): search() for the global quick-search endpoint

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
  EOF
  )"
  ```

---

## Task 4: Ctrl-K overlay in chaos-ui

New module `crates/chaos-ui/src/search.rs` with the `QuickSearch` overlay component plus two pure helpers (`flatten`, `step`) that get native unit tests (chaos-ui compiles natively; pattern: `components.rs` tests). The App registers a window-level keydown listener for Ctrl-K/Cmd-K with `on_cleanup` (exact pattern of the existing `external_clicks` listener in `lib.rs`), adds a topbar search button and a tabbar search button (mobile). The overlay is its own lightweight dialog (the shared `Modal` centers vertically and carries a title bar; a command palette wants top-anchored, title-less chrome — but it reuses the same backdrop click-to-close/stop-propagation idiom). Input is debounced ~250ms with the `set_timeout` + equality-check idiom; results come from a `LocalResource` over `ChaosClient::search`; ArrowUp/Down move a selection cursor over the flattened hits, Enter activates, Escape closes. Activation: events navigate to `/calendar`; anything with a URL goes through `crate::open_external` (shells) falling back to `window.open(_blank)` (browser).

**Files:**
- Create: `/projects/rust/chaos/crates/chaos-ui/src/search.rs`
- Modify: `/projects/rust/chaos/crates/chaos-ui/src/lib.rs` (module, Ctrl-K listener, overlay mount, topbar + tabbar buttons)
- Modify: `/projects/rust/chaos/crates/chaos-web/styles.css` (overlay + button styles)

**Steps:**

- [ ] Create `/projects/rust/chaos/crates/chaos-ui/src/search.rs` with the pure helpers and their tests FIRST:

  ```rust
  //! Global quick-search overlay (Ctrl-K / Cmd-K): debounced input, grouped
  //! results, arrow-key navigation. Mounted once at App level; opened from
  //! the keyboard shortcut or the topbar/tabbar search buttons.

  use std::time::Duration;

  use chaos_domain::{SearchHit, SearchKind, SearchResults};
  use leptos::prelude::*;

  use crate::use_client;

  /// Order-preserving flattening of the grouped results: index N in the
  /// flat list is the Nth rendered row, so one cursor spans all groups.
  fn flatten(results: &SearchResults) -> Vec<SearchHit> {
      results
          .services
          .iter()
          .chain(&results.bookmarks)
          .chain(&results.links)
          .chain(&results.events)
          .cloned()
          .collect()
  }

  /// Move the selection cursor by `dir` (±1), wrapping at both ends.
  fn step(current: usize, len: usize, dir: isize) -> usize {
      if len == 0 {
          return 0;
      }
      (current.min(len - 1) as isize + dir).rem_euclid(len as isize) as usize
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      fn hit(kind: SearchKind, title: &str) -> SearchHit {
          SearchHit {
              kind,
              title: title.into(),
              subtitle: None,
              url: None,
          }
      }

      #[test]
      fn flatten_preserves_group_order_for_keyboard_navigation() {
          let results = SearchResults {
              services: vec![hit(SearchKind::Service, "Jellyfin")],
              bookmarks: vec![hit(SearchKind::Bookmark, "GitHub")],
              links: vec![
                  hit(SearchKind::Link, "Rust blog"),
                  hit(SearchKind::Link, "Leptos"),
              ],
              events: vec![hit(SearchKind::Event, "Dentist")],
          };
          let titles: Vec<_> = flatten(&results)
              .iter()
              .map(|h| h.title.clone())
              .collect();
          assert_eq!(titles, ["Jellyfin", "GitHub", "Rust blog", "Leptos", "Dentist"]);
          assert!(flatten(&SearchResults::default()).is_empty());
      }

      #[test]
      fn step_wraps_in_both_directions_and_survives_empty_lists() {
          assert_eq!(step(0, 3, 1), 1);
          assert_eq!(step(2, 3, 1), 0);
          assert_eq!(step(0, 3, -1), 2);
          assert_eq!(step(0, 0, 1), 0);
          // Cursor beyond the list (results shrank) clamps before stepping.
          assert_eq!(step(9, 3, 1), 0);
      }
  }
  ```

  And declare the module in `/projects/rust/chaos/crates/chaos-ui/src/lib.rs`:

  ```rust
  mod search;
  ```
  after `mod pages;`.

- [ ] Run and verify both tests pass (they are pure; if `step` were wrong they'd fail — to make the failure visible under strict TDD, run once with `step` returning `current` unchanged and watch `step_wraps_in_both_directions_and_survives_empty_lists` fail on `assert_eq!(step(0, 3, 1), 1)`, then restore):

  ```
  cargo test -p chaos-ui search::tests -- --nocapture
  ```

- [ ] Add the `QuickSearch` component to `/projects/rust/chaos/crates/chaos-ui/src/search.rs` (below `step`, above the tests):

  ```rust
  #[component]
  pub fn QuickSearch(open: RwSignal<bool>) -> impl IntoView {
      let query = RwSignal::new(String::new());
      let debounced = RwSignal::new(String::new());
      let selected = RwSignal::new(0usize);
      let input_ref = NodeRef::<leptos::html::Input>::new();

      // ~250ms debounce: forward the query only once it stopped changing.
      Effect::new(move |_| {
          let q = query.get();
          set_timeout(
              move || {
                  if query.get_untracked() == q {
                      debounced.set(q);
                  }
              },
              Duration::from_millis(250),
          );
      });

      let client = use_client();
      let results = LocalResource::new(move || {
          let q = debounced.get();
          let client = client.clone();
          async move {
              if q.trim().is_empty() {
                  return Ok(SearchResults::default());
              }
              client.search(&q).await
          }
      });

      // New results reset the cursor; opening focuses the input.
      Effect::new(move |_| {
          debounced.track();
          selected.set(0);
      });
      Effect::new(move |_| {
          if open.get()
              && let Some(input) = input_ref.get()
          {
              let _ = input.focus();
          }
      });

      let close = move || {
          open.set(false);
          query.set(String::new());
          debounced.set(String::new());
          selected.set(0);
      };

      let navigate = leptos_router::hooks::use_navigate();
      let activate = Callback::new(move |hit: SearchHit| {
          match hit.kind {
              // Events carry no URL of their own; land on the calendar.
              SearchKind::Event => navigate("/calendar", Default::default()),
              _ => {
                  if let Some(url) = &hit.url
                      && !crate::open_external(url.as_str())
                      && let Some(window) = web_sys::window()
                  {
                      let _ = window.open_with_url_and_target(url.as_str(), "_blank");
                  }
              }
          }
          close();
      });

      let keydown = move |ev: leptos::ev::KeyboardEvent| {
          let flat = results
              .get_untracked()
              .and_then(|r| r.ok())
              .map(|r| flatten(&r))
              .unwrap_or_default();
          match ev.key().as_str() {
              "ArrowDown" => {
                  ev.prevent_default();
                  selected.update(|s| *s = step(*s, flat.len(), 1));
              }
              "ArrowUp" => {
                  ev.prevent_default();
                  selected.update(|s| *s = step(*s, flat.len(), -1));
              }
              "Enter" => {
                  if let Some(hit) = flat.into_iter().nth(selected.get_untracked()) {
                      activate.run(hit);
                  }
              }
              "Escape" => close(),
              _ => {}
          }
      };

      view! {
          {move || {
              open.get()
                  .then(|| {
                      view! {
                          <div class="quick-search-backdrop" on:click=move |_| close()>
                              <div class="quick-search" on:click=|ev| ev.stop_propagation()>
                                  <input
                                      class="quick-search-input"
                                      type="search"
                                      placeholder="Search services, bookmarks, links, events…"
                                      prop:value=query
                                      on:input=move |ev| query.set(event_target_value(&ev))
                                      on:keydown=keydown
                                      node_ref=input_ref
                                  />
                                  <div class="quick-search-results">
                                      {move || match results.get() {
                                          None => view! { <p class="muted">"Searching…"</p> }.into_any(),
                                          Some(Err(err)) => {
                                              view! { <p class="muted">{format!("Search failed: {err}")}</p> }
                                                  .into_any()
                                          }
                                          Some(Ok(res)) => {
                                              if flatten(&res).is_empty() {
                                                  let label = if query.get_untracked().trim().is_empty() {
                                                      "Type to search"
                                                  } else {
                                                      "No results"
                                                  };
                                                  return view! { <p class="muted">{label}</p> }.into_any();
                                              }
                                              let mut index = 0usize;
                                              [
                                                  ("Services", res.services),
                                                  ("Bookmarks", res.bookmarks),
                                                  ("Links", res.links),
                                                  ("Events", res.events),
                                              ]
                                                  .into_iter()
                                                  .filter(|(_, hits)| !hits.is_empty())
                                                  .map(|(label, hits)| {
                                                      view! {
                                                          <div class="qs-group">
                                                              <h4 class="muted">{label}</h4>
                                                              {hits
                                                                  .into_iter()
                                                                  .map(|hit| {
                                                                      let i = index;
                                                                      index += 1;
                                                                      let title = hit.title.clone();
                                                                      let subtitle =
                                                                          hit.subtitle.clone().unwrap_or_default();
                                                                      view! {
                                                                          <button
                                                                              class="qs-row"
                                                                              class:selected=move || selected.get() == i
                                                                              on:click=move |_| activate.run(hit.clone())
                                                                          >
                                                                              <span class="qs-title">{title}</span>
                                                                              <span class="qs-sub muted">{subtitle}</span>
                                                                          </button>
                                                                      }
                                                                  })
                                                                  .collect_view()}
                                                          </div>
                                                      }
                                                  })
                                                  .collect_view()
                                                  .into_any()
                                          }
                                      }}
                                  </div>
                              </div>
                          </div>
                      }
                  })
          }}
      }
  }
  ```

- [ ] Wire it in `/projects/rust/chaos/crates/chaos-ui/src/lib.rs`. Inside `App`, right after the theme `Effect::new(move |_| apply_theme(...))` line, add:

  ```rust
      // Global quick-search: Ctrl-K (Cmd-K on mac) toggles the overlay from
      // anywhere. Window-level listener, removed on unmount like the click
      // interceptor below.
      let search_open = RwSignal::new(false);
      let search_keys = window_event_listener(leptos::ev::keydown, move |ev| {
          if (ev.ctrl_key() || ev.meta_key()) && ev.key().eq_ignore_ascii_case("k") {
              ev.prevent_default();
              search_open.update(|o| *o = !*o);
          }
      });
      on_cleanup(move || search_keys.remove());
  ```

  In the `view!` block: after `<ShareRedirect/>` add

  ```rust
              <search::QuickSearch open=search_open/>
  ```

  In the topbar, right after the `<A href="/home">…</A>` entry (before `<span class="topbar-foot">`), add:

  ```rust
                  <button
                      class="topbar-search"
                      title="Search (Ctrl-K)"
                      on:click=move |_| search_open.set(true)
                  >
                      <span class="nav-icon">"⌕"</span>
                      "Search"
                      <kbd>"Ctrl K"</kbd>
                  </button>
  ```

  In the tabbar (mobile), after the `NAV_PRIMARY … .collect_view()` expression, add:

  ```rust
                  <button class="tab-search" on:click=move |_| search_open.set(true)>
                      <span class="tab-icon">"⌕"</span>
                      <span class="tab-label">"Search"</span>
                  </button>
  ```

- [ ] Append the styles to `/projects/rust/chaos/crates/chaos-web/styles.css`:

  ```css
  /* ---- quick-search (Ctrl-K) ---- */
  .quick-search-backdrop {
    position: fixed;
    inset: 0;
    z-index: 60;
    background: rgba(0, 0, 0, 0.55);
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 12vh;
  }
  .quick-search {
    width: min(560px, 92vw);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 10px;
    overflow: hidden;
  }
  .quick-search-input {
    width: 100%;
    padding: 0.8rem 1rem;
    border: none;
    border-bottom: 1px solid var(--border);
    background: transparent;
    color: var(--text);
    font-size: 1rem;
  }
  .quick-search-input:focus {
    outline: none;
  }
  .quick-search-results {
    max-height: 55vh;
    overflow-y: auto;
    padding: 0.4rem;
  }
  .quick-search-results > p {
    margin: 0.6rem;
  }
  .qs-group h4 {
    margin: 0.4rem 0.6rem 0.2rem;
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .qs-row {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 1rem;
    width: 100%;
    padding: 0.45rem 0.6rem;
    background: none;
    border: none;
    border-radius: 6px;
    color: var(--text);
    text-align: left;
    cursor: pointer;
    font: inherit;
  }
  .qs-row:hover,
  .qs-row.selected {
    background: var(--bg);
  }
  .qs-row.selected {
    outline: 1px solid var(--accent);
  }
  .qs-sub {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 0.8rem;
  }
  .topbar-search {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    background: none;
    border: none;
    color: var(--muted);
    cursor: pointer;
    padding: 0.5rem 0.75rem;
    font: inherit;
    text-align: left;
  }
  .topbar-search:hover {
    color: var(--text);
  }
  .topbar-search kbd {
    margin-left: auto;
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 0.3rem;
    font-size: 0.7rem;
    color: var(--muted);
  }
  .tabbar .tab-search {
    background: none;
    border: none;
    color: var(--muted);
    display: flex;
    flex-direction: column;
    align-items: center;
    font: inherit;
    cursor: pointer;
  }
  ```

  Note: the topbar is the desktop sidebar/topbar and the tabbar is the mobile bottom bar — check the existing `.tabbar a` rules in the mobile media query (`styles.css` ~line 1356) and mirror their font-size/padding on `.tab-search` if the button visibly diverges from its `<a>` siblings.

- [ ] Verify: native tests green, native + wasm compilation clean:

  ```
  cargo test -p chaos-ui search::tests -- --nocapture
  cargo check -p chaos-ui
  cargo check -p chaos-web -p chaos-ui --target wasm32-unknown-unknown
  ```

- [ ] Manual smoke test (optional but recommended): `just server` + `just web`, open http://127.0.0.1:8080, press Ctrl-K, type "jelly", check groups render, arrows move the highlight, Enter opens, Escape closes; click the tabbar Search button at a narrow viewport.

- [ ] Commit:

  ```
  cd /projects/rust/chaos
  git add crates/chaos-ui crates/chaos-web/styles.css
  git commit -m "$(cat <<'EOF'
  feat(ui): global quick-search overlay on Ctrl-K

  Debounced input, grouped results, arrow-key navigation + Enter to
  open (events land on /calendar); topbar and tabbar buttons for
  mouse/touch. Window keydown listener removed via on_cleanup.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
  EOF
  )"
  ```

---

## Task 5: Scheduled SQLite backup

`[backup]` config section (opt-in: `enabled = false` by default; `dir` defaults to `backups`, resolved against the working directory exactly like `db_path`/`archive.dir` — `/var/lib/chaos` under the NixOS module) and a background task modeled on `archiver::spawn`. Each cycle runs `VACUUM INTO '<dir>/chaos-<timestamp>.db'` through the sqlx pool — `VACUUM INTO` is a plain statement, works under WAL without blocking writers, but takes a filename literal (no bind parameter), so single quotes in the path are SQL-escaped. Then old files are pruned to the `keep` newest; timestamped names sort chronologically so pruning is a pure, unit-testable function over filenames. All failures are logged, never fatal.

**Files:**
- Create: `/projects/rust/chaos/crates/chaos-server/src/backup.rs`
- Modify: `/projects/rust/chaos/crates/chaos-server/src/config.rs` (`BackupConfig`)
- Modify: `/projects/rust/chaos/crates/chaos-server/src/main.rs` (`mod backup;` + spawn)

**Steps:**

- [ ] Add the failing config test to the `tests` module in `/projects/rust/chaos/crates/chaos-server/src/config.rs`:

  ```rust
      #[test]
      fn backup_section_parses_and_defaults_are_sane() {
          let toml = r#"
              [backup]
              enabled = true
          "#;
          let config: super::Config = figment::Figment::from(
              figment::providers::Serialized::defaults(super::Config::default()),
          )
          .merge(figment::providers::Toml::string(toml))
          .extract()
          .expect("backup section must parse");
          assert!(config.backup.enabled);
          assert_eq!(config.backup.dir, std::path::PathBuf::from("backups"));
          assert_eq!(config.backup.interval_hours, 24);
          assert_eq!(config.backup.keep, 14);
          assert!(
              !super::Config::default().backup.enabled,
              "backups are opt-in"
          );
      }
  ```

- [ ] Run and verify it FAILS to compile (`no field backup on type Config`):

  ```
  cargo test -p chaos-server config::tests::backup_section_parses_and_defaults_are_sane -- --exact
  ```

- [ ] Implement `BackupConfig` in `/projects/rust/chaos/crates/chaos-server/src/config.rs`. Add the field to `Config` (after `pub archive: ArchiveConfig,`):

  ```rust
      /// Scheduled database backups (`VACUUM INTO` snapshots). Off unless
      /// `enabled = true`.
      pub backup: BackupConfig,
  ```

  Add the struct next to `ArchiveConfig`:

  ```rust
  #[derive(Debug, Clone, Serialize, Deserialize)]
  #[serde(default)]
  pub struct BackupConfig {
      /// Master switch for the scheduled backup task.
      pub enabled: bool,
      /// Directory where snapshots land (`chaos-<timestamp>.db`). Relative
      /// paths resolve against the working directory (`/var/lib/chaos`
      /// under the NixOS module), like `db_path` and `archive.dir`.
      pub dir: PathBuf,
      /// Hours between two snapshots (clamped to at least 1).
      pub interval_hours: u64,
      /// How many snapshots to keep; older ones are pruned after each run.
      pub keep: usize,
  }

  impl Default for BackupConfig {
      fn default() -> Self {
          Self {
              enabled: false,
              dir: PathBuf::from("backups"),
              interval_hours: 24,
              keep: 14,
          }
      }
  }
  ```

  And extend `impl Default for Config` with `backup: BackupConfig::default(),` (after `archive: ArchiveConfig::default(),`).

- [ ] Run green:

  ```
  cargo test -p chaos-server config::tests::backup_section_parses_and_defaults_are_sane -- --exact
  ```

- [ ] Create `/projects/rust/chaos/crates/chaos-server/src/backup.rs` (tests included — under strict TDD, paste the `tests` module first, watch it fail to compile against the missing `backup_once`/`to_delete`, then add the implementation):

  ```rust
  //! Scheduled SQLite backups: every `interval_hours`, `VACUUM INTO` a
  //! timestamped copy of the database, then prune to the `keep` newest
  //! files. `VACUUM INTO` produces a consistent, defragmented snapshot
  //! without blocking writers (works under WAL), so no downtime is needed.
  //! Failures are logged and retried next cycle — never fatal.

  use std::path::{Path, PathBuf};
  use std::time::Duration;

  use chrono::Utc;

  use crate::db::Db;
  use crate::state::AppState;

  pub fn spawn(state: AppState) {
      if state.config.backup.enabled {
          tokio::spawn(run(state));
      }
  }

  async fn run(state: AppState) {
      let dir = state.config.backup.dir.clone();
      if let Err(err) = tokio::fs::create_dir_all(&dir).await {
          tracing::error!(
              dir = %dir.display(),
              %err,
              "cannot create backup dir; backups disabled"
          );
          return;
      }
      let interval = Duration::from_secs(state.config.backup.interval_hours.max(1) * 3600);
      let keep = state.config.backup.keep.max(1);

      loop {
          match backup_once(&state.db, &dir).await {
              Ok(path) => tracing::info!(path = %path.display(), "database backed up"),
              Err(err) => tracing::error!(%err, "database backup failed"),
          }
          if let Err(err) = prune(&dir, keep).await {
              tracing::warn!(%err, "pruning old backups failed");
          }
          tokio::time::sleep(interval).await;
      }
  }

  /// Write one consistent snapshot of the live database into `dir` and
  /// return its path. `VACUUM INTO` takes a filename literal, not a bind
  /// parameter, so single quotes in the path are SQL-escaped.
  pub async fn backup_once(db: &Db, dir: &Path) -> anyhow::Result<PathBuf> {
      let name = format!("chaos-{}.db", Utc::now().format("%Y%m%d-%H%M%S"));
      let path = dir.join(name);
      let escaped = path.display().to_string().replace('\'', "''");
      sqlx::query(&format!("VACUUM INTO '{escaped}'"))
          .execute(&db.pool)
          .await?;
      Ok(path)
  }

  /// Delete everything beyond the `keep` newest backups.
  async fn prune(dir: &Path, keep: usize) -> anyhow::Result<()> {
      let mut names = Vec::new();
      let mut entries = tokio::fs::read_dir(dir).await?;
      while let Some(entry) = entries.next_entry().await? {
          if let Ok(name) = entry.file_name().into_string() {
              names.push(name);
          }
      }
      for name in to_delete(names, keep) {
          tracing::info!(file = name, "pruning old backup");
          tokio::fs::remove_file(dir.join(name)).await?;
      }
      Ok(())
  }

  /// The backup files to delete: everything except the `keep` newest.
  /// Lexicographic order == chronological for `chaos-<timestamp>.db`
  /// names; files not matching the pattern are never touched.
  fn to_delete(mut names: Vec<String>, keep: usize) -> Vec<String> {
      names.retain(|n| n.starts_with("chaos-") && n.ends_with(".db"));
      names.sort();
      let cut = names.len().saturating_sub(keep);
      names.truncate(cut);
      names
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use chaos_domain::CreateLinkRequest;

      #[test]
      fn to_delete_keeps_the_newest_and_ignores_foreign_files() {
          let names = vec![
              "chaos-20260701-000000.db".to_string(),
              "chaos-20260703-000000.db".to_string(),
              "chaos-20260702-000000.db".to_string(),
              "notes.txt".to_string(),
              "live.db".to_string(),
          ];
          assert_eq!(
              to_delete(names.clone(), 2),
              vec!["chaos-20260701-000000.db"]
          );
          assert!(to_delete(names.clone(), 3).is_empty());
          assert!(to_delete(names, 10).is_empty());
          assert!(to_delete(vec![], 5).is_empty());
      }

      #[tokio::test]
      async fn backup_once_produces_an_openable_consistent_copy() {
          // File-backed live db in a scratch dir; VACUUM INTO lands beside it.
          let dir = std::env::temp_dir().join(format!("chaos-backup-test-{}", uuid::Uuid::now_v7()));
          std::fs::create_dir_all(&dir).unwrap();
          let db = Db::connect(&dir.join("live.db")).await.unwrap();

          let link = db
              .create_link(
                  &CreateLinkRequest {
                      url: "https://example.com/backup".parse().unwrap(),
                      title: Some("kept across backup".into()),
                      description: None,
                      collection_id: None,
                      tags: vec![],
                  },
                  false,
                  None,
              )
              .await
              .unwrap();

          let path = backup_once(&db, &dir).await.unwrap();
          assert!(path.exists());
          assert!(path.file_name().unwrap().to_str().unwrap().starts_with("chaos-"));

          // The snapshot is a complete database: opening it (re-runs the
          // migration check) and reading the link back must work.
          let restored = Db::connect(&path).await.unwrap();
          assert_eq!(
              restored.get_link(link.id).await.unwrap().title,
              "kept across backup"
          );

          std::fs::remove_dir_all(&dir).unwrap();
      }
  }
  ```

- [ ] Run and verify both tests pass (and, if pasted tests-first, that they failed to compile before the implementation existed):

  ```
  cargo test -p chaos-server backup::tests -- --nocapture
  ```

- [ ] Wire it into `/projects/rust/chaos/crates/chaos-server/src/main.rs`: add `mod backup;` to the module list (after `mod auth;`), and spawn it next to the other tasks:

  ```rust
      monitor::spawn(state.clone());
      archiver::spawn(state.clone());
      backup::spawn(state.clone());
  ```

- [ ] Full verification:

  ```
  cargo nextest run -p chaos-server
  just check
  ```

- [ ] Commit:

  ```
  cd /projects/rust/chaos
  git add crates/chaos-server
  git commit -m "$(cat <<'EOF'
  feat(server): scheduled SQLite backups via VACUUM INTO

  Opt-in [backup] config: timestamped snapshots every interval_hours
  into backup.dir, pruned to the keep newest. Consistent under WAL,
  no downtime; failures log and retry next cycle.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
  EOF
  )"
  ```

---

## Task 6: Docs, example config, roadmap

**Files:**
- Modify: `/projects/rust/chaos/crates/chaos-server/chaos.example.toml`
- Modify: `/projects/rust/chaos/docs/deployment.md`
- Modify: `/projects/rust/chaos/docs/ROADMAP.md`

**Steps:**

- [ ] In `/projects/rust/chaos/crates/chaos-server/chaos.example.toml`, after the `[archive]` block, add:

  ```toml
  # Scheduled database backups: a consistent snapshot (VACUUM INTO) of the
  # SQLite file every interval, pruned to the `keep` newest. Off by default.
  # [backup]
  # enabled = true
  # dir = "backups"        # chaos-<timestamp>.db files land here
  # interval_hours = 24
  # keep = 14
  ```

- [ ] In `/projects/rust/chaos/docs/deployment.md`, add a short section before "## Desktop and phone apps":

  ```markdown
  ## Backups

  Enable scheduled SQLite backups with the `[backup]` section
  (`services.chaos.settings.backup` on NixOS):

  ```nix
  services.chaos.settings.backup = {
    enabled = true;
    # dir defaults to "backups" → /var/lib/chaos/backups under the module.
    interval_hours = 24;
    keep = 14;
  };
  ```

  Each run writes a consistent `chaos-<timestamp>.db` snapshot via SQLite's
  `VACUUM INTO` (safe under WAL, no downtime) and prunes to the `keep`
  newest. Restoring = stopping the service and copying a snapshot over
  `db_path`. Page archives (`[archive] dir`) and icons are plain files —
  include `/var/lib/chaos` in the host's regular backup for those.
  ```

- [ ] In `/projects/rust/chaos/docs/ROADMAP.md`, flip the two Phase 8 checkboxes:

  ```markdown
  - [x] Global quick-search across services, links and events (Ctrl-K):
        `GET /api/v1/search` + overlay (debounced, grouped, arrow-key nav)
  - [x] Scheduled SQLite backup/export: `[backup]` config, VACUUM INTO
        snapshots with retention pruning
  ```

- [ ] Sanity check the example config still parses:

  ```
  CHAOS_CONFIG=crates/chaos-server/chaos.example.toml cargo run -p chaos-server -- list-users
  ```

  (Exits after printing users — proves config load; run from `/projects/rust/chaos`.)

- [ ] Final full gate:

  ```
  just test
  just check
  ```

- [ ] Commit:

  ```
  cd /projects/rust/chaos
  git add crates/chaos-server/chaos.example.toml docs/deployment.md docs/ROADMAP.md
  git commit -m "$(cat <<'EOF'
  docs: backup config example, deployment notes, roadmap checkboxes

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
  EOF
  )"
  ```

---

## Notes for the executor

- **Route table:** all routes really are in `api/mod.rs::router()` (no services.rs/widgets.rs split ever landed); `dashboard`/`services`/`widget_data` handlers sit in `api/mod.rs` itself.
- **Type consistency:** `SearchHit.url` is `Option<url::Url>` end to end (domain, server hits, client, UI `hit.url.as_str()`); events always ship `url: None` and the UI routes them to `/calendar` by `kind`.
- **Auth semantics:** `/search` is registered without any extractor gate — `optional_user_id` (bearer header or `chaos_session` cookie) only feeds the events group, mirroring how `links::create` attributes anonymously.
- **`VACUUM INTO` + sqlx:** plain `sqlx::query(...).execute(&pool)` works; it cannot run inside a transaction (we don't) and the target file must not already exist (timestamped names make collisions a same-second edge only; a failure just logs and retries next cycle).
- **Leptos idioms used:** `window_event_listener` + `on_cleanup` (existing pattern in `App`), `set_timeout` equality-check debounce (existing `set_timeout` usage in `pages/links.rs`), `LocalResource` for fetches, `Callback` for shared activation, `NodeRef` focus effect. `let`-chains (`if let ... && let ...`) are already used in this codebase (edition 2024).
- If `cargo test -p <crate> <path> -- --exact` doesn't find a test under nextest habits, `cargo nextest run -p <crate> -E 'test(<name>)'` is equivalent; both are available.
