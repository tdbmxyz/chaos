# Correctness Bug Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix four verified data-correctness bugs: (1) calendar event editing silently wipes the description, (2) Home Assistant temperature history attributes readings to the wrong sensors when HA omits an entity, (3) LIKE-pattern escaping in link search forgets to escape backslash, and (4) non-ASCII usernames can never log in because lookups rely on SQLite's ASCII-only `COLLATE NOCASE`.

**Architecture:** Rust cargo workspace. `chaos-server` is an axum backend over SQLite (sqlx); its tests use `Db::in_memory()` and axum-based stub Home Assistant servers bound to `127.0.0.1:0`. `chaos-ui` is a Leptos (`csr`) component library that compiles natively as well as for wasm — it already has native `#[cfg(test)]` unit tests (e.g. `pages/home.rs`), run by the workspace test suite. `chaos-domain` holds the shared types (`CalendarEvent`, `TemperatureSeries`, `LinkQuery`, ...). Each task is TDD: failing test first, minimal fix, green, commit.

**Tech Stack:** Rust, axum, sqlx (SQLite), tokio, serde, chrono, Leptos 0.8, cargo-nextest (`just test` runs `cargo nextest run --workspace`).

---

## Task 1: Calendar event edit wipes the description

The event dialog's `EventDraft::edit()` at `crates/chaos-ui/src/pages/calendar.rs:89` fills `description: String::new()` instead of copying `event.description`. Event updates are full-replacement PUTs, so editing a title (or anything else) deletes the stored description.

`EventDraft` is plain data and `chaos-ui` compiles natively (leptos `csr`; `pages/home.rs` already carries native `#[cfg(test)]` unit tests run by `cargo nextest run --workspace`), so a native unit test works. `calendar.rs` has no tests module yet — add one at the end of the file (current last line: 817). `Utc`, `TimeZone`, `Uuid`, and `CalendarEvent` are already imported at the top of the file, so `use super::*;` suffices.

**Files:**
- Modify: `/projects/rust/chaos/crates/chaos-ui/src/pages/calendar.rs` (line 89 fix; new tests module appended at end of file)
- Test: same file, new `#[cfg(test)] mod tests`

**Steps:**

- [ ] Append the failing test to the end of `/projects/rust/chaos/crates/chaos-ui/src/pages/calendar.rs`:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      /// Editing must load every event field into the draft: updates are
      /// full-replacement PUTs, so a field the draft drops is a field the
      /// save deletes.
      #[test]
      fn edit_carries_over_the_description() {
          let event = CalendarEvent {
              id: Some(Uuid::now_v7()),
              calendar_id: Uuid::now_v7(),
              calendar_name: "Perso".into(),
              color: None,
              title: "Dentist".into(),
              description: Some("Bring the x-rays".into()),
              location: Some("12 rue des Lilas".into()),
              starts_at: Utc.with_ymd_and_hms(2026, 7, 11, 9, 0, 0).unwrap(),
              ends_at: Utc.with_ymd_and_hms(2026, 7, 11, 10, 0, 0).unwrap(),
              all_day: false,
          };

          let draft = EventDraft::edit(&event);

          assert_eq!(draft.title, "Dentist");
          assert_eq!(draft.location, "12 rue des Lilas");
          assert_eq!(
              draft.description, "Bring the x-rays",
              "editing an event must not blank its description"
          );
      }
  }
  ```

- [ ] Run the test and verify it fails on the description assertion (left: `""`, right: `"Bring the x-rays"`):

  ```
  cargo test -p chaos-ui pages::calendar::tests::edit_carries_over_the_description -- --exact
  ```

- [ ] Apply the minimal fix in `EventDraft::edit` (line 89). Old:

  ```rust
              description: String::new(),
  ```

  New:

  ```rust
              description: event.description.clone().unwrap_or_default(),
  ```

  (This is the line inside `fn edit`, between `location: event.location.clone().unwrap_or_default(),` and `all_day: event.all_day,` — `fn new_on` at line 63 keeps its `description: String::new(),`.)

- [ ] Verify the test passes, and that the wasm target still compiles:

  ```
  cargo test -p chaos-ui pages::calendar::tests::edit_carries_over_the_description -- --exact
  cargo check -p chaos-ui --target wasm32-unknown-unknown
  ```

- [ ] Commit:

  ```
  git add crates/chaos-ui/src/pages/calendar.rs
  git commit -m "fix(calendar): keep the description when editing an event

  EventDraft::edit filled description with String::new() instead of the
  event's stored description; since saves are full-replacement PUTs,
  editing any field silently deleted the description.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP"
  ```

---

## Task 2: Home Assistant temperature history mislabels sensors

`temperature_history` (`crates/chaos-server/src/home_assistant.rs:145-155`) zips the response arrays of `GET /api/history/period` to `self.sensors` by position. But HA omits entities that have no states in the window, so every array after an omitted entity lands on the wrong sensor (the `raw.resize_with` only pads the tail). Fix: with `minimal_response`, HA keeps `entity_id` on the **first** entry of each array — deserialize it (`HaStateChange` at line 391-395 gains an optional `entity_id`) and match arrays to sensors by entity id, padding misses with empty reading lists.

**Files:**
- Modify: `/projects/rust/chaos/crates/chaos-server/src/home_assistant.rs`
  - `HaStateChange` struct (lines 391-395): add `entity_id: Option<String>`
  - `temperature_history` (lines 145-170): match by entity id instead of position
- Test: same file, `mod tests` (starts line 410) — new stub `stub_ha_history`, new sensor helper, new test

**Steps:**

- [ ] Add the failing test to the `tests` module of `/projects/rust/chaos/crates/chaos-server/src/home_assistant.rs`, after `stub_ha_single_entity` and its tests (matches the existing stub-HA style — axum router on `127.0.0.1:0`, `client_with` for token plumbing):

  ```rust
      fn sensor_def(id: &str, entity_id: &str) -> HomeEntityDef {
          HomeEntityDef {
              id: id.into(),
              label: Some(id.into()),
              entity_id: entity_id.into(),
              battery_entity_id: None,
          }
      }

      /// Stub HA answering `/api/history/period/{start}` with a fixed JSON
      /// body, whatever the query string.
      async fn stub_ha_history(body: serde_json::Value) -> Url {
          let app = axum::Router::new().route(
              "/api/history/period/{start}",
              axum::routing::get(move |_: Path<String>| {
                  let body = body.clone();
                  async move { axum::Json(body) }
              }),
          );
          let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
              .await
              .expect("binding stub ha");
          let addr = listener.local_addr().expect("stub ha addr");
          tokio::spawn(async move {
              axum::serve(listener, app).await.expect("serving stub ha");
          });
          format!("http://{addr}/").parse().expect("stub ha url")
      }

      /// HA omits entities with no states in the window. Three sensors are
      /// requested but the middle one ("b") has no history: its array is
      /// absent from the response. Positional matching would hand b the
      /// readings of c — series must be matched by the `entity_id` carried
      /// on the first entry of each array instead.
      #[tokio::test]
      async fn history_matches_series_by_entity_id_when_ha_omits_one() {
          use chrono::TimeZone;

          // minimal_response: only the first entry of each array carries
          // `entity_id`; later entries are state + last_changed only.
          let body = serde_json::json!([
              [
                  {
                      "entity_id": "sensor.a_temperature",
                      "state": "20.0",
                      "last_changed": "2026-07-11T10:00:00+00:00"
                  },
                  { "state": "20.5", "last_changed": "2026-07-11T11:00:00+00:00" }
              ],
              [
                  {
                      "entity_id": "sensor.c_temperature",
                      "state": "23.0",
                      "last_changed": "2026-07-11T10:30:00+00:00"
                  }
              ]
          ]);
          let url = stub_ha_history(body).await;
          let ha = client_with(
              url,
              vec![
                  sensor_def("a", "sensor.a_temperature"),
                  sensor_def("b", "sensor.b_temperature"),
                  sensor_def("c", "sensor.c_temperature"),
              ],
              vec![],
          );

          let start = Utc.with_ymd_and_hms(2026, 7, 11, 0, 0, 0).unwrap();
          let end = Utc.with_ymd_and_hms(2026, 7, 12, 0, 0, 0).unwrap();
          let series = ha.temperature_history(start, end).await.expect("history");

          assert_eq!(series.len(), 3, "one series per configured sensor");
          assert_eq!(series[0].id, "a");
          assert_eq!(series[0].readings.len(), 2);
          assert_eq!(series[0].readings[1].celsius, 20.5);
          assert_eq!(series[1].id, "b");
          assert!(
              series[1].readings.is_empty(),
              "an entity HA omitted must come back empty, not steal the next sensor's readings"
          );
          assert_eq!(series[2].id, "c");
          assert_eq!(series[2].readings.len(), 1);
          assert_eq!(series[2].readings[0].celsius, 23.0);
      }
  ```

- [ ] Run it and verify it fails on the `series[1].readings.is_empty()` assertion (positional zip gives b the 23.0 reading meant for c, and c comes back empty):

  ```
  cargo test -p chaos-server home_assistant::tests::history_matches_series_by_entity_id_when_ha_omits_one -- --exact
  ```

- [ ] Fix `HaStateChange` (lines 391-395). Old:

  ```rust
  #[derive(Debug, Deserialize)]
  struct HaStateChange {
      state: String,
      last_changed: DateTime<Utc>,
  }
  ```

  New:

  ```rust
  #[derive(Debug, Deserialize)]
  struct HaStateChange {
      /// With `minimal_response`, present only on the first entry of each
      /// per-entity array — that one entry identifies the whole array.
      #[serde(default)]
      entity_id: Option<String>,
      state: String,
      last_changed: DateTime<Utc>,
  }
  ```

- [ ] Fix `temperature_history` (lines 145-170; `HashMap` is already imported at the top of the file). Old:

  ```rust
          // One array per requested entity, in request order (per HA's docs).
          // `minimal_response` drops `entity_id` (and attributes) from every
          // entry but the first for a given entity, so match by array
          // position instead of by `entity_id`.
          let mut raw: Vec<Vec<HaStateChange>> = self.get_json(url).await?;
          raw.resize_with(self.sensors.len(), Vec::new);

          let mut series = Vec::with_capacity(self.sensors.len());
          for (def, changes) in self.sensors.iter().zip(raw) {
              series.push(TemperatureSeries {
                  id: def.id.clone(),
                  label: self.label(def).await,
                  readings: changes
                      .into_iter()
                      .filter_map(|change| {
                          // "unavailable"/"unknown" don't parse as a number.
                          let celsius = change.state.parse::<f64>().ok()?;
                          Some(TemperatureReading {
                              at: change.last_changed,
                              celsius,
                          })
                      })
                      .collect(),
              });
          }
          Ok(series)
  ```

  New:

  ```rust
          // One array per entity — but HA omits entities with no states in
          // the window, so matching by position would shift every later
          // array onto the wrong sensor. `minimal_response` keeps
          // `entity_id` on the first entry of each array: match on that,
          // and pad omitted entities with an empty reading list.
          let raw: Vec<Vec<HaStateChange>> = self.get_json(url).await?;
          let mut by_entity: HashMap<String, Vec<HaStateChange>> = HashMap::new();
          for changes in raw {
              if let Some(entity_id) = changes.first().and_then(|c| c.entity_id.clone()) {
                  by_entity.insert(entity_id, changes);
              }
          }

          let mut series = Vec::with_capacity(self.sensors.len());
          for def in &self.sensors {
              let changes = by_entity.remove(&def.entity_id).unwrap_or_default();
              series.push(TemperatureSeries {
                  id: def.id.clone(),
                  label: self.label(def).await,
                  readings: changes
                      .into_iter()
                      .filter_map(|change| {
                          // "unavailable"/"unknown" don't parse as a number.
                          let celsius = change.state.parse::<f64>().ok()?;
                          Some(TemperatureReading {
                              at: change.last_changed,
                              celsius,
                          })
                      })
                      .collect(),
              });
          }
          Ok(series)
  ```

- [ ] Verify the new test passes and nothing else in the module regressed:

  ```
  cargo test -p chaos-server home_assistant::tests::history_matches_series_by_entity_id_when_ha_omits_one -- --exact
  cargo test -p chaos-server home_assistant::
  ```

- [ ] Commit:

  ```
  git add crates/chaos-server/src/home_assistant.rs
  git commit -m "fix(home): match HA history arrays to sensors by entity_id

  /api/history/period omits entities with no states in the window, so
  zipping response arrays to sensors by position shifted every later
  series onto the wrong sensor. minimal_response keeps entity_id on the
  first entry of each array; match on that and pad misses with empty
  reading lists.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP"
  ```

---

## Task 3: LIKE escaping misses backslash

`list_links` (`crates/chaos-server/src/db.rs:436`) builds the LIKE pattern with `q.replace('%', "\\%").replace('_', "\\_")` under `ESCAPE '\'` — but never escapes `\` itself. A user searching for `\` produces the pattern `%\%%`, where `\%` is an *escaped percent*: the search matches titles containing a literal `%` instead of titles containing a backslash.

**Files:**
- Modify: `/projects/rust/chaos/crates/chaos-server/src/db.rs` (line 436)
- Test: same file, `mod tests` (starts line 690) — uses the existing `Db::in_memory()` + `link_req`-style setup

**Steps:**

- [ ] Add the failing test to the `tests` module of `/projects/rust/chaos/crates/chaos-server/src/db.rs` (e.g. after `link_crud_with_tags_and_orphan_gc`; `CreateLinkRequest`, `LinkQuery`, `Db` are already in scope via `use super::*;`):

  ```rust
      /// LIKE metacharacters in the search query must match literally:
      /// `%`, `_`, and — the regression — `\` itself. An unescaped `\`
      /// under ESCAPE '\' turns the following `%` into a literal percent,
      /// so searching for a backslash finds percent signs instead.
      #[tokio::test]
      async fn search_escapes_like_metacharacters_literally() {
          let db = Db::in_memory().await.unwrap();

          let titled = |title: &str, url: &str| CreateLinkRequest {
              url: url.parse().unwrap(),
              title: Some(title.into()),
              description: None,
              collection_id: None,
              tags: vec![],
          };
          db.create_link(&titled("50% off", "https://example.com/percent"), false, None)
              .await
              .unwrap();
          db.create_link(&titled("c:\\temp", "https://example.com/backslash"), false, None)
              .await
              .unwrap();
          db.create_link(&titled("my_var", "https://example.com/underscore"), false, None)
              .await
              .unwrap();
          db.create_link(&titled("myxvar", "https://example.com/decoy"), false, None)
              .await
              .unwrap();

          let search = |q: &str| LinkQuery {
              q: Some(q.into()),
              ..Default::default()
          };

          // A lone backslash must find "c:\temp", not the "%" title.
          let backslash = db.list_links(&search("\\")).await.unwrap();
          assert_eq!(backslash.total, 1, "backslash must match literally");
          assert_eq!(backslash.items[0].title, "c:\\temp");

          // Literal `%`: only the percent title, not everything.
          let percent = db.list_links(&search("50%")).await.unwrap();
          assert_eq!(percent.total, 1);
          assert_eq!(percent.items[0].title, "50% off");

          // Literal `_`: must not wildcard-match "myxvar".
          let underscore = db.list_links(&search("my_var")).await.unwrap();
          assert_eq!(underscore.total, 1);
          assert_eq!(underscore.items[0].title, "my_var");
      }
  ```

- [ ] Run it and verify it fails on the backslash **title** assertion: pre-fix, the query `\` builds the pattern `%\%%` where `\%` is an escaped percent, which matches exactly one link — the wrong one — so `backslash.total` is 1 but `backslash.items[0].title` is `"50% off"` instead of `"c:\temp"`:

  ```
  cargo test -p chaos-server db::tests::search_escapes_like_metacharacters_literally -- --exact
  ```

- [ ] Apply the minimal fix at line 436 — escape `\` **first**, so the escapes added for `%`/`_` aren't themselves re-escaped. Old:

  ```rust
                  let pattern = format!("%{}%", q.replace('%', "\\%").replace('_', "\\_"));
  ```

  New:

  ```rust
                  let pattern = format!(
                      "%{}%",
                      q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
                  );
  ```

- [ ] Verify the test passes and the rest of the db tests stay green:

  ```
  cargo test -p chaos-server db::tests::search_escapes_like_metacharacters_literally -- --exact
  cargo test -p chaos-server db::
  ```

- [ ] Commit:

  ```
  git add crates/chaos-server/src/db.rs
  git commit -m "fix(links): escape backslash in LIKE search patterns

  The search pattern escaped % and _ but not \\ itself, so under
  ESCAPE '\\' a query containing a backslash re-purposed the next
  character: searching for \\ matched literal percent signs instead of
  backslashes. Escape \\ first so the added escapes survive.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP"
  ```

---

## Task 4: Non-ASCII usernames can never log in

`create_user` (`crates/chaos-server/src/db_auth.rs:20`) normalizes with Rust's Unicode-aware `to_lowercase()` — `"Émile"` is stored as `"émile"`. But `user_by_username` (line 54-62) and `user_with_password` (line 65-74) bind the raw trimmed input and lean on SQLite's `COLLATE NOCASE`, which folds **ASCII only**: `'É' != 'é'` to NOCASE, so a user who registers as `Émile` and types `Émile` (or `ÉMILE`) at login never matches the stored row. Fix: lowercase the lookup argument in Rust in both functions, exactly like `create_user` does.

**Files:**
- Modify: `/projects/rust/chaos/crates/chaos-server/src/db_auth.rs` (lines 57 and 68, the `.bind(...)` in both lookup functions)
- Test: same file, `mod tests` (starts line 158)

**Steps:**

- [ ] Add the failing test to the `tests` module of `/projects/rust/chaos/crates/chaos-server/src/db_auth.rs`, after `user_and_session_lifecycle`:

  ```rust
      /// SQLite's COLLATE NOCASE only folds ASCII, but create_user
      /// normalizes with Rust's Unicode to_lowercase(): "Émile" is stored
      /// as "émile". Lookups must fold in Rust too, or accented usernames
      /// can never log back in.
      #[tokio::test]
      async fn non_ascii_usernames_are_looked_up_case_insensitively() {
          let db = Db::in_memory().await.expect("db");
          let user = db
              .create_user("Émile", "Émile", "phc-string")
              .await
              .expect("user");
          assert_eq!(user.username, "émile");

          let found = db.user_by_username("ÉMILE").await.expect("lookup by name");
          assert_eq!(found.id, user.id);

          let (found, hash) = db
              .user_with_password("Émile")
              .await
              .expect("login lookup");
          assert_eq!(found.id, user.id);
          assert_eq!(hash, "phc-string");
      }
  ```

- [ ] Run it and verify it fails with `Err(NotFound)` on the `user_by_username("ÉMILE")` expect:

  ```
  cargo test -p chaos-server db_auth::tests::non_ascii_usernames_are_looked_up_case_insensitively -- --exact
  ```

- [ ] Apply the minimal fix: lowercase the bound argument in both lookup functions (`COLLATE NOCASE` stays — it is harmless and still covers rows predating Unicode normalization). In `user_by_username`, old:

  ```rust
                  .bind(username.trim())
  ```

  New:

  ```rust
                  .bind(username.trim().to_lowercase())
  ```

  In `user_with_password`, old:

  ```rust
                  .bind(username.trim())
  ```

  New:

  ```rust
                  .bind(username.trim().to_lowercase())
  ```

  (The two lines are textually identical — either use `replace_all` for both occurrences of `.bind(username.trim())` in the file, or include surrounding query lines to disambiguate. Both occurrences must change; no other function in the file binds a username.)

- [ ] Verify the new test and the existing lifecycle test (which covers the ASCII `"TIBO"` lookup) both pass:

  ```
  cargo test -p chaos-server db_auth::
  ```

- [ ] Run the full workspace suite as a final gate (same as CI / `just test`):

  ```
  cargo nextest run --workspace
  ```

- [ ] Commit:

  ```
  git add crates/chaos-server/src/db_auth.rs
  git commit -m "fix(auth): fold username lookups in Rust, not COLLATE NOCASE

  create_user stores usernames through Unicode to_lowercase(), but the
  login lookups bound the raw input and relied on SQLite's ASCII-only
  NOCASE collation — so accented usernames like Émile could register
  but never log in. Lowercase the lookup argument in Rust in
  user_by_username and user_with_password.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP"
  ```
