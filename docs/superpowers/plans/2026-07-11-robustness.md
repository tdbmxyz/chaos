# Server Robustness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix verified robustness issues in `chaos-server`: unbounded/buffered-then-checked HTTP body reads, an archiver that can OOM on huge monolith output or hot-loop on persistent DB errors, a UTF-8 slice panic in ICS date parsing, silent recurrence truncation, and an in-memory test DB pool whose extra connections are independent empty databases. A shared `http_util` module (capped body streaming + JSON GET) is introduced first and then adopted by the fetchers.

**Architecture:** New module `crates/chaos-server/src/http_util.rs` with three functions: `get_json<T>` (GET + status check + JSON), `get_body_capped` (GET + status check + streamed body that errors past a cap), and the extracted, stream-generic `read_capped` (unit-testable without HTTP). Widget fetchers (`weather.rs`, `posts.rs`, `feed.rs`, `releases.rs`) and `ics.rs` switch to these helpers. `home_assistant.rs::get_json` is deliberately left alone: it is a method carrying bearer auth from `self.token`, and parameterizing auth into the shared helper for one caller adds surface without payoff. `metadata.rs` also stays as is: it intentionally *truncates* at its cap (metadata lives in `<head>`), while feeds/ICS must *fail* on oversized bodies — different semantics, same streaming pattern. Archiver and DB fixes are local hardening.

**Tech Stack:** Rust (workspace edition), tokio, reqwest (`stream` feature already enabled), axum (used for in-crate stub servers, pattern from `home_assistant.rs` tests), futures, sqlx/SQLite, ical/rrule/chrono, feed-rs.

---

Conventions used throughout:

- All commands run from `/projects/rust/chaos`.
- Test convention in this crate: `#[cfg(test)] mod tests` at the bottom of each source file; async tests use `#[tokio::test]`; HTTP is stubbed with a local axum router on `127.0.0.1:0` (see `home_assistant.rs:433-465`).
- Commit messages follow the repo's conventional-commit style (`fix(...)`, `feat(...)`) and every commit ends with these exact trailers:

  ```
  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
  ```

---

## Task 1: Shared capped-body HTTP helper (`http_util.rs`)

**Files:**
- Create: `crates/chaos-server/src/http_util.rs`
- Edit: `crates/chaos-server/src/main.rs` (module list, lines 1-14)

Duplicated helpers being replaced later: `widgets/weather.rs:175-184`, `widgets/posts.rs:98-107`. Correct streaming pattern copied from `metadata.rs:64-73`. Error strings deliberately do NOT embed the URL — `widgets/feed.rs:23` and `widgets/releases.rs:23` already prefix errors with the URL/repo at collection time, and embedding it in the helper would duplicate it.

**Steps:**

- [ ] Write the new module with failing-to-compile tests first. Create `crates/chaos-server/src/http_util.rs`:

  ```rust
  //! Shared HTTP fetch helpers for widget providers and feed subscriptions.
  //!
  //! Two rules every remote fetch must follow: check the status before the
  //! body, and never buffer an unbounded body — `get_body_capped` streams and
  //! fails as soon as the cap is crossed, so a hostile or misconfigured URL
  //! cannot balloon server memory.

  use futures::StreamExt;

  /// GET `url` and deserialize the JSON body. Non-2xx statuses become errors.
  pub async fn get_json<T: serde::de::DeserializeOwned>(
      http: &reqwest::Client,
      url: &str,
  ) -> Result<T, String> {
      let resp = http.get(url).send().await.map_err(|e| e.to_string())?;
      if !resp.status().is_success() {
          return Err(format!("status {}", resp.status()));
      }
      resp.json().await.map_err(|e| e.to_string())
  }

  /// GET `url` and return the raw body, erroring as soon as it exceeds
  /// `max_bytes` — the body is streamed, never buffered past the cap.
  pub async fn get_body_capped(
      http: &reqwest::Client,
      url: &str,
      max_bytes: usize,
  ) -> Result<Vec<u8>, String> {
      let resp = http.get(url).send().await.map_err(|e| e.to_string())?;
      if !resp.status().is_success() {
          return Err(format!("status {}", resp.status()));
      }
      read_capped(resp.bytes_stream(), max_bytes).await
  }

  /// Accumulate a chunk stream, failing once the total would pass `max_bytes`.
  /// Split out from `get_body_capped` so the cap logic is testable with a
  /// synthetic stream.
  async fn read_capped<S, B, E>(stream: S, max_bytes: usize) -> Result<Vec<u8>, String>
  where
      S: futures::Stream<Item = Result<B, E>>,
      B: AsRef<[u8]>,
      E: std::fmt::Display,
  {
      let mut stream = std::pin::pin!(stream);
      let mut body: Vec<u8> = Vec::new();
      while let Some(chunk) = stream.next().await {
          let chunk = chunk.map_err(|e| format!("reading body: {e}"))?;
          let chunk = chunk.as_ref();
          if body.len() + chunk.len() > max_bytes {
              return Err(format!("body exceeds {max_bytes} bytes"));
          }
          body.extend_from_slice(chunk);
      }
      Ok(body)
  }

  #[cfg(test)]
  mod tests {
      use serde::Deserialize;

      use super::*;

      #[derive(Debug, Deserialize, PartialEq)]
      struct Payload {
          answer: u32,
      }

      /// One-route stub server (pattern from home_assistant.rs tests):
      /// always answers `status` + `body`. Returns the full URL.
      async fn stub(status: axum::http::StatusCode, body: &'static str) -> String {
          let app = axum::Router::new().route(
              "/data",
              axum::routing::get(move || async move { (status, body) }),
          );
          let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
              .await
              .expect("binding stub");
          let addr = listener.local_addr().expect("stub addr");
          tokio::spawn(async move {
              axum::serve(listener, app).await.expect("serving stub");
          });
          format!("http://{addr}/data")
      }

      #[tokio::test]
      async fn get_json_deserializes_a_success_response() {
          let url = stub(axum::http::StatusCode::OK, r#"{"answer":42}"#).await;
          let got: Payload = get_json(&reqwest::Client::new(), &url)
              .await
              .expect("json");
          assert_eq!(got, Payload { answer: 42 });
      }

      #[tokio::test]
      async fn get_json_reports_http_errors_before_parsing() {
          let url = stub(axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom").await;
          let err = get_json::<Payload>(&reqwest::Client::new(), &url)
              .await
              .expect_err("5xx must fail");
          assert!(err.contains("500"), "unexpected error: {err}");
      }

      #[tokio::test]
      async fn get_body_capped_returns_bodies_under_the_cap() {
          let url = stub(axum::http::StatusCode::OK, "hello").await;
          let body = get_body_capped(&reqwest::Client::new(), &url, 1024)
              .await
              .expect("body");
          assert_eq!(body, b"hello");
      }

      #[tokio::test]
      async fn get_body_capped_rejects_oversized_bodies() {
          let url = stub(axum::http::StatusCode::OK, "hello world").await;
          let err = get_body_capped(&reqwest::Client::new(), &url, 4)
              .await
              .expect_err("cap must trip");
          assert!(err.contains("exceeds 4 bytes"), "unexpected error: {err}");
      }

      #[tokio::test]
      async fn read_capped_stops_mid_stream_at_the_cap() {
          let chunks: Vec<Result<&[u8], String>> =
              vec![Ok(b"aaaa"), Ok(b"bbbb"), Ok(b"cccc")];
          let err = read_capped(futures::stream::iter(chunks), 6)
              .await
              .expect_err("third chunk crosses the cap on the second");
          assert!(err.contains("exceeds 6 bytes"), "unexpected error: {err}");
      }

      #[tokio::test]
      async fn read_capped_accumulates_streams_that_fit() {
          let chunks: Vec<Result<&[u8], String>> = vec![Ok(b"aaaa"), Ok(b"bb")];
          let body = read_capped(futures::stream::iter(chunks), 6)
              .await
              .expect("exactly at the cap is fine");
          assert_eq!(body, b"aaaabb");
      }

      #[tokio::test]
      async fn read_capped_surfaces_stream_errors() {
          let chunks: Vec<Result<&[u8], String>> =
              vec![Ok(b"aa"), Err("reset by peer".into())];
          let err = read_capped(futures::stream::iter(chunks), 1024)
              .await
              .expect_err("stream error must propagate");
          assert!(err.contains("reset by peer"), "unexpected error: {err}");
      }
  }
  ```

- [ ] Run `cargo test -p chaos-server http_util` — expect a **compile error** (`http_util` is not yet a module of the crate; nothing references it), confirming the tests cannot pass yet.

- [ ] Register the module. In `crates/chaos-server/src/main.rs`, the module list currently reads `mod home_assistant;` / `mod ics;` on lines 8-9; insert alphabetically:

  ```rust
  mod home_assistant;
  mod http_util;
  mod ics;
  ```

- [ ] Note: `get_json` and `get_body_capped` will trigger `dead_code` warnings until Task 2 wires them in (`read_capped` is exercised by its own tests). If the build treats warnings as errors here, add `#[allow(dead_code)] // used from widgets as of the next commit` on the two pub functions and remove it in Task 2; otherwise proceed.

- [ ] Run `cargo test -p chaos-server http_util` — expect all 7 tests to pass (`test result: ok. 7 passed`).

- [ ] Run `cargo test -p chaos-server` — expect the full suite green (no existing behavior touched).

- [ ] Commit:

  ```bash
  git add crates/chaos-server/src/http_util.rs crates/chaos-server/src/main.rs
  git commit -m "$(cat <<'EOF'
  feat(server): shared http_util with capped body streaming

  get_json centralizes the GET + status-check + JSON pattern duplicated in
  the weather and posts widgets; get_body_capped streams response bodies
  and fails at the cap instead of buffering first (pattern lifted from
  metadata.rs). read_capped is split out and unit-tested with synthetic
  streams; the HTTP paths are tested against a local axum stub.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
  EOF
  )"
  ```

## Task 2: Adopt the helpers — fix buffered-then-checked caps, add the missing one

**Files:**
- Edit: `crates/chaos-server/src/widgets/feed.rs` (lines 37-48: `fetch_one` buffers via `resp.bytes()` then checks `MAX_BODY_BYTES`)
- Edit: `crates/chaos-server/src/ics.rs` (lines 113-123: `FeedCache::fetch`, same buffered-then-checked pattern; tests at 289-350)
- Edit: `crates/chaos-server/src/widgets/releases.rs` (lines 41-47: no cap at all)
- Edit: `crates/chaos-server/src/widgets/posts.rs` (delete local `get_json`, lines 98-107)
- Edit: `crates/chaos-server/src/widgets/weather.rs` (delete local `get_json`, lines 175-184)

Behavior notes (intended, harmless): oversized-feed errors change from `"feed(/feed body) too large (N bytes)"` to `"body exceeds N bytes"`; weather HTTP errors change from `"open-meteo returned 500 ..."` to `"status 500 ..."`; posts errors drop the URL prefix (`feed.rs`/`releases.rs` callers still prefix with URL/repo, so no information is lost where it aggregates). No existing test asserts these strings.

**Steps:**

- [ ] Add a failing test to `crates/chaos-server/src/ics.rs` (inside the existing `mod tests`, after `range_filters_out_of_window_occurrences`). It stubs a server returning an oversized body and asserts the *streaming* error message, which the current buffer-then-check code (`"feed too large"`) does not produce:

  ```rust
      /// Serves `body` at /feed.ics on an ephemeral port; returns the URL.
      async fn stub_feed(body: Vec<u8>) -> String {
          let app = axum::Router::new().route(
              "/feed.ics",
              axum::routing::get(move || {
                  let body = body.clone();
                  async move { body }
              }),
          );
          let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
              .await
              .expect("binding stub feed");
          let addr = listener.local_addr().expect("stub feed addr");
          tokio::spawn(async move {
              axum::serve(listener, app).await.expect("serving stub feed");
          });
          format!("http://{addr}/feed.ics")
      }

      #[tokio::test]
      async fn fetch_rejects_oversized_feeds_while_streaming() {
          let url = stub_feed(vec![b' '; MAX_BODY_BYTES + 1]).await;
          let err = FeedCache::default()
              .fetch(&url)
              .await
              .expect_err("oversized feed must fail");
          assert!(err.contains("exceeds"), "unexpected error: {err}");
      }
  ```

- [ ] Run `cargo test -p chaos-server ics::tests::fetch_rejects_oversized_feeds_while_streaming` — expect **FAILED** with the assertion message showing the old error `feed too large (8388609 bytes)`.

- [ ] Switch `ics.rs` to the helper. Replace `FeedCache::fetch` (lines 113-123):

  ```rust
      async fn fetch(&self, url: &str) -> Result<Vec<RawEvent>, String> {
          let body = crate::http_util::get_body_capped(&self.http, url, MAX_BODY_BYTES).await?;
          parse(&body)
      }
  ```

- [ ] Run `cargo test -p chaos-server ics` — expect all ics tests to pass, including the new one.

- [ ] Switch `widgets/feed.rs`. Replace the body of `fetch_one` (lines 37-47) up to the `feed_rs` parse:

  ```rust
  async fn fetch_one(http: &reqwest::Client, url: Url) -> Result<Vec<FeedItem>, String> {
      let body = crate::http_util::get_body_capped(http, url.as_str(), MAX_BODY_BYTES).await?;
      let feed = feed_rs::parser::parse(body.as_slice()).map_err(|e| e.to_string())?;
  ```

  (the rest of the function, from `let source = ...`, is unchanged). The `MAX_BODY_BYTES` const on line 10 stays.

- [ ] Add the missing cap to `widgets/releases.rs`. Add below the imports (line 5):

  ```rust
  /// Cap on a fetched releases.atom body; GitHub's feeds are a few KB, so
  /// anything near this is a misbehaving response.
  const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
  ```

  and replace lines 42-47 of `latest_release`:

  ```rust
      let url = format!("https://github.com/{repo}/releases.atom");
      let body = crate::http_util::get_body_capped(http, &url, MAX_BODY_BYTES).await?;
      let feed = feed_rs::parser::parse(body.as_slice()).map_err(|e| e.to_string())?;
  ```

  (The URL is hardcoded to github.com, so this path is not stub-testable without a base-URL refactor that is out of scope; the cap behavior itself is covered by the Task 1 helper tests and the ics test above.)

- [ ] Deduplicate `get_json`. In `widgets/posts.rs`: delete the local `get_json` (lines 98-107) and add `use crate::http_util::get_json;` to the imports. In `widgets/weather.rs`: delete the local `get_json` (lines 175-184) and add `use crate::http_util::get_json;` to the imports. If Task 1 added `#[allow(dead_code)]` markers in `http_util.rs`, remove them now.

- [ ] Run `cargo test -p chaos-server` — expect the full suite green (posts mapping tests, ics tests, weather tests all unchanged in behavior).

- [ ] Commit:

  ```bash
  git add crates/chaos-server/src/ics.rs crates/chaos-server/src/widgets/feed.rs crates/chaos-server/src/widgets/releases.rs crates/chaos-server/src/widgets/posts.rs crates/chaos-server/src/widgets/weather.rs crates/chaos-server/src/http_util.rs
  git commit -m "$(cat <<'EOF'
  fix(server): stream remote bodies and stop at the cap

  feed.rs and ics.rs buffered the whole response before checking
  MAX_BODY_BYTES, so a hostile feed could balloon memory before the check;
  both now use http_util::get_body_capped which fails mid-stream.
  releases.atom had no cap at all and gets one. posts/weather drop their
  copy-pasted get_json for the shared helper.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
  EOF
  )"
  ```

## Task 3: Archiver hardening — snapshot size cap and no hot-loop

**Files:**
- Edit: `crates/chaos-server/src/archiver.rs`
  - `finalize` (lines 124-142): `read_to_string` of the monolith output with no size check → OOM risk
  - `run` loop (lines 43-50): `continue` even when `finish_archive` fails → a persistent DB/disk error re-fetches the same pending link and re-runs monolith in a tight loop
  - tests module (lines 175-188): currently only `extract_text`; `finalize` is a private fn in the same file, so it is directly testable with temp files

**Steps:**

- [ ] Add failing tests for the size cap to the `tests` module in `archiver.rs`:

  ```rust
      /// Fresh temp dir per test so parallel tests don't collide.
      async fn temp_dir(name: &str) -> std::path::PathBuf {
          let dir = std::env::temp_dir().join(format!(
              "chaos-archiver-{name}-{}",
              std::process::id()
          ));
          let _ = tokio::fs::remove_dir_all(&dir).await;
          tokio::fs::create_dir_all(&dir).await.expect("temp dir");
          dir
      }

      #[tokio::test]
      async fn finalize_rejects_oversized_snapshots() {
          let dir = temp_dir("oversized").await;
          let tmp = dir.join("page.html.tmp");
          let final_path = dir.join("page.html");
          // Sparse file: instant to create, but metadata reports the full size.
          let file = tokio::fs::File::create(&tmp).await.expect("create tmp");
          file.set_len(MAX_SNAPSHOT_BYTES + 1).await.expect("set_len");
          drop(file);

          let err = finalize(&tmp, &final_path)
              .await
              .expect_err("oversized snapshot must fail the archive");
          assert!(err.contains("too large"), "unexpected reason: {err}");
          // The snapshot must not have been moved into place.
          assert!(tokio::fs::metadata(&final_path).await.is_err());
          let _ = tokio::fs::remove_dir_all(&dir).await;
      }

      #[tokio::test]
      async fn finalize_moves_small_snapshots_and_extracts_text() {
          let dir = temp_dir("small").await;
          let tmp = dir.join("page.html.tmp");
          let final_path = dir.join("page.html");
          let html = "<html><body><p>hello world</p></body></html>";
          tokio::fs::write(&tmp, html).await.expect("write tmp");

          let (size_bytes, text) = finalize(&tmp, &final_path).await.expect("finalize");
          assert_eq!(size_bytes, html.len() as u64);
          assert_eq!(text, "hello world");
          assert!(tokio::fs::metadata(&final_path).await.is_ok());
          assert!(tokio::fs::metadata(&tmp).await.is_err());
          let _ = tokio::fs::remove_dir_all(&dir).await;
      }
  ```

- [ ] Run `cargo test -p chaos-server archiver` — expect a **compile error**: `MAX_SNAPSHOT_BYTES` does not exist yet (the small-snapshot test alone would pass; the constant is the missing piece).

- [ ] Implement the cap. In `archiver.rs`, add below `MAX_FTS_TEXT_BYTES` (line 20):

  ```rust
  /// Snapshots larger than this fail the archive instead of being read into
  /// memory; monolith can emit hundreds of MB for pathological pages and an
  /// unchecked read_to_string would balloon the server.
  const MAX_SNAPSHOT_BYTES: u64 = 64 * 1024 * 1024;
  ```

  and change the top of `finalize` (lines 128-131) from the current read-then-measure to check-then-read:

  ```rust
      let size_bytes = tokio::fs::metadata(tmp_path)
          .await
          .map_err(|e| format!("inspecting snapshot: {e}"))?
          .len();
      if size_bytes > MAX_SNAPSHOT_BYTES {
          return Err(format!(
              "snapshot too large ({size_bytes} bytes, cap {MAX_SNAPSHOT_BYTES})"
          ));
      }
      let html = tokio::fs::read_to_string(tmp_path)
          .await
          .map_err(|e| format!("reading snapshot: {e}"))?;
  ```

  Delete the now-redundant `let size_bytes = html.len() as u64;` line. (`archive_one` already routes any `Err` from `finalize` to `ArchiveOutcome::Failure` + tmp-file cleanup, lines 84-91 — no changes needed there.)

- [ ] Run `cargo test -p chaos-server archiver` — expect all archiver tests to pass (3 total).

- [ ] Fix the hot-loop. In `run` (lines 43-50), replace the `Ok(Some(link))` arm so a `finish_archive` error falls through to the idle `tokio::select!` sleep instead of `continue`-ing straight back into `next_pending_archive` (which would return the same still-pending link and re-run monolith immediately, forever):

  ```rust
              Ok(Some(link)) => {
                  let outcome = archive_one(&state, &link).await;
                  match state.db.finish_archive(link.id, outcome).await {
                      // Drain the queue before sleeping.
                      Ok(()) => continue,
                      // Fall through to the idle sleep: if the outcome can't
                      // be recorded the link stays pending, and looping now
                      // would re-run monolith on it in a tight loop.
                      Err(err) => {
                          tracing::error!(link = %link.id, %err, "recording archive outcome");
                      }
                  }
              }
  ```

  This arm is a background loop wired to `AppState` + a live DB and is not covered by the existing tests (none exercise `run`); it is verified by review and by the full suite staying green. The testable half of this task (the size cap) has tests above.

- [ ] Run `cargo test -p chaos-server` — expect the full suite green.

- [ ] Commit:

  ```bash
  git add crates/chaos-server/src/archiver.rs
  git commit -m "$(cat <<'EOF'
  fix(server): cap archiver snapshot size and stop hot-looping on DB errors

  finalize now checks the snapshot's on-disk size (64 MB cap) before
  read_to_string, failing the archive with a reason instead of loading a
  huge monolith output into memory. When finish_archive itself errors the
  worker falls through to the idle sleep instead of continue, so a
  persistent DB/disk failure can't re-run monolith in a tight loop.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
  EOF
  )"
  ```

## Task 4: ICS — multibyte panic fix, truncation visibility, all-day comment

**Files:**
- Edit: `crates/chaos-server/src/ics.rs`
  - `parse_datetime` (line 207): `&raw[..8.min(raw.len())]` panics if byte 8 splits a multibyte UTF-8 char
  - `expand` (lines 266-269): `all(MAX_OCCURRENCES)` truncation is silent (`result.limited: bool` on rrule 0.14's `RRuleResult` reports it)
  - All-day semantics: DATE values are pinned to UTC midnight (line 208) — known limitation, explicitly **out of scope**; comment only

**Steps:**

- [ ] Add a failing test to `ics.rs`'s `mod tests` (`ical::property::Property` has all-pub fields `name`, `params`, `value` — verified against ical 0.11):

  ```rust
      #[test]
      fn parse_datetime_survives_multibyte_date_values() {
          // 9 bytes: byte index 8 falls inside the two-byte 'é', so a naive
          // `&raw[..8]` slice panics. Must return None instead.
          let prop = ical::property::Property {
              name: "DTSTART".into(),
              params: Some(vec![("VALUE".into(), vec!["DATE".into()])]),
              value: Some("2026071é".into()),
          };
          assert!(parse_datetime(&prop).is_none());
      }
  ```

- [ ] Run `cargo test -p chaos-server ics::tests::parse_datetime_survives_multibyte_date_values` — expect **FAILED** with a panic: `byte index 8 is not a char boundary`.

- [ ] Fix the slice and document the all-day limitation. Replace the `is_date` branch (lines 206-209):

  ```rust
      if is_date {
          // Known limitation: all-day events are pinned to UTC midnight, so
          // viewers in negative-UTC-offset zones see them start a day early.
          // Fixing this means plumbing a display timezone through the whole
          // calendar API — deliberately out of scope here.
          let date = raw
              .get(..8)
              .and_then(|s| NaiveDate::parse_from_str(s, "%Y%m%d").ok())?;
          return Some((Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0)?), true));
      }
  ```

  (Behavior is preserved for short values: `raw.get(..8)` is `None` when `raw` is shorter than 8 bytes, where the old code sliced short and then failed the parse — both yield `None`.)

- [ ] Run `cargo test -p chaos-server ics` — expect all ics tests to pass, including the new one.

- [ ] Make truncation visible. In `expand`, after the `.all(MAX_OCCURRENCES)` call (line 269) and before the `for date in result.dates` loop, add:

  ```rust
      if result.limited {
          tracing::debug!(
              title = event.title,
              limit = MAX_OCCURRENCES,
              "recurrence expansion truncated at MAX_OCCURRENCES"
          );
      }
  ```

- [ ] Run `cargo test -p chaos-server` — expect the full suite green.

- [ ] Commit:

  ```bash
  git add crates/chaos-server/src/ics.rs
  git commit -m "$(cat <<'EOF'
  fix(server): ics date parsing can't panic on multibyte values

  parse_datetime sliced &raw[..8] which panics when byte 8 splits a
  multibyte UTF-8 char in a hostile or corrupt feed; raw.get(..8) degrades
  to skipping the event. Recurrence expansion now logs at debug level when
  MAX_OCCURRENCES truncates, and the all-day UTC-midnight limitation is
  documented in place (fix out of scope).

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
  EOF
  )"
  ```

## Task 5: In-memory DB pool — one connection, honest comments

**Files:**
- Edit: `crates/chaos-server/src/db.rs`
  - `in_memory` (lines 60-68): the pool may open up to 4 `:memory:` connections, and each SQLite `:memory:` connection is an *independent empty database* — only the first one gets migrated, so tests that hit a second pooled connection see `no such table`
  - `with_options` (lines 70-78): the comment claims "A single writer avoids SQLITE_BUSY surprises" while configuring `max_connections(4)` — reconcile the comment with reality (WAL + sqlx's default 5s `busy_timeout`); keep `max_connections(4)` for file DBs

**Steps:**

- [ ] Add a failing test to `db.rs`'s `mod tests` (near the existing archive-queue tests around line 846; `futures` is already a crate dependency):

  ```rust
      #[tokio::test]
      async fn in_memory_pool_is_one_shared_database() {
          let db = Db::in_memory().await.unwrap();
          // Concurrent queries force the pool to open extra connections when
          // max_connections > 1. Every extra `:memory:` connection is a
          // fresh, unmigrated database, so one of these fails with
          // "no such table: collections" until the pool is capped at 1.
          let results =
              futures::future::join_all((0..8).map(|_| db.list_collections())).await;
          for result in results {
              result.expect("every pooled connection must see the migrated schema");
          }
      }
  ```

- [ ] Run `cargo test -p chaos-server db::tests::in_memory_pool_is_one_shared_database` — expect **FAILED** (`no such table: collections` from a freshly opened second connection). Note: the failure depends on the pool opening a second connection under contention; 8 concurrent acquires against a 4-connection pool makes this reliable in practice. If it ever passes spuriously, re-run — it must fail before the fix and pass deterministically after.

- [ ] Implement. Replace `in_memory` and `with_options` (lines 60-78):

  ```rust
      /// In-memory database for tests. Every SQLite `:memory:` connection is
      /// its own independent database, so the pool must be capped at one
      /// connection — a second one would be fresh and unmigrated.
      #[cfg(test)]
      pub async fn in_memory() -> Result<Self> {
          use std::str::FromStr;
          let options = SqliteConnectOptions::from_str("sqlite::memory:")
              .expect("valid memory dsn")
              .foreign_keys(true);
          Self::with_options(options, 1).await
      }

      async fn with_options(options: SqliteConnectOptions, max_connections: u32) -> Result<Self> {
          let pool = SqlitePoolOptions::new()
              // SQLite still allows only one writer at a time; with WAL,
              // reads on the other pooled connections run concurrently and a
              // contended writer waits on sqlx's default 5s busy_timeout
              // instead of failing with SQLITE_BUSY.
              .max_connections(max_connections)
              .connect_with(options)
              .await?;
          sqlx::migrate!("./migrations").run(&pool).await?;
          Ok(Self { pool })
      }
  ```

  and update the call in `connect` (line 57) to `Self::with_options(options, 4).await`.

- [ ] Run `cargo test -p chaos-server db::tests::in_memory_pool_is_one_shared_database` — expect **ok**.

- [ ] Run the full server suite as required: `cargo test -p chaos-server` — expect everything green (every DB-backed test uses `Db::in_memory`, so this exercises the single-connection pool broadly; watch for any test that would deadlock by holding a pooled connection while issuing a second query — none exist today).

- [ ] Commit:

  ```bash
  git add crates/chaos-server/src/db.rs
  git commit -m "$(cat <<'EOF'
  fix(server): single-connection pool for in-memory test databases

  Each sqlite :memory: connection is an independent empty database, so the
  4-connection test pool could hand out unmigrated databases under
  concurrency. Cap the in-memory pool at 1 connection; file-backed pools
  keep 4, with the comment corrected to describe the real WAL +
  busy_timeout semantics instead of claiming a single writer.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
  EOF
  )"
  ```

## Final verification

- [ ] `cargo test -p chaos-server` — full suite green.
- [ ] `cargo test --workspace` — no other crate touches the changed modules, but confirm.
- [ ] `cargo clippy -p chaos-server -- -D warnings` and `cargo fmt --check` — clean (in particular: no leftover `#[allow(dead_code)]` in `http_util.rs`, no unused imports left behind in `posts.rs`/`weather.rs`/`feed.rs`/`releases.rs`/`ics.rs`).
- [ ] Grep guard: `grep -rn "resp.bytes()" crates/chaos-server/src` must return no matches (metadata.rs already streams via `bytes_stream`; feed/ics/releases now go through the helper).
