# Plan B — Viewed-State + Analytics UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On the authed `/news` page + reader, mark posts seen (viewport), opened-comments (title tap / reader), opened-article (favicon / reader link), render the five states, and record + sync all events (plus `app_open`/`reader_open`) through an offline outbox.

**Architecture:** A new `chaos-ui/src/analytics.rs` module owns a localStorage outbox, a global optimistic overlay signal, a debounced + reconnect flush, and the `app_open` throttle. `NewsPage` loads the server viewed-map, provides a `ViewedState` context, and runs an IntersectionObserver to mark seen. `post_row_view` reads the context to dim/mark rows and record click events. The reader records opened-comments + reader_open on load.

**Tech Stack:** Leptos 0.8 CSR, web_sys (IntersectionObserver, localStorage), chaos-client, chaos-domain. Depends on: Plan A merged. Spec: `docs/superpowers/specs/2026-07-22-news-viewed-state-analytics-design.md`.

**Verification (every task):**
- `cargo test -p chaos-ui`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`
- `cargo check -p chaos-ui --target wasm32-unknown-unknown`

Commit UNSIGNED (`git -c commit.gpgsign=false`), one per task, standard trailers:
`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
`Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP`
Do NOT push. Leave the dirty schema files untouched.

---

### Task B1: Pure analytics helpers

**Files:**
- Create: `crates/chaos-ui/src/analytics.rs`
- Modify: `crates/chaos-ui/src/lib.rs` (`mod analytics;`)

- [ ] **Step 1: Write failing tests** for the pure logic (no DOM/localStorage):

```rust
#[test]
fn app_open_throttle() {
    let now = 1_000_000i64; // seconds
    assert!(should_record_app_open(None, now));                 // never opened
    assert!(should_record_app_open(Some(now - 301), now));      // > 5 min ago
    assert!(!should_record_app_open(Some(now - 299), now));     // < 5 min ago
}

#[test]
fn merge_flags_is_or() {
    let a = ViewFlags { seen: true, comments: false, article: false };
    let b = ViewFlags { seen: false, comments: true, article: false };
    assert_eq!(merge_flags(a, b), ViewFlags { seen: true, comments: true, article: false });
}

#[test]
fn row_state_class_derivation() {
    let none = ViewFlags::default();
    let seen = ViewFlags { seen: true, ..none };
    let read = ViewFlags { seen: true, comments: true, ..none };
    let article = ViewFlags { seen: true, article: true, ..none };
    let both = ViewFlags { seen: true, comments: true, article: true };
    assert_eq!(row_state_class(none), "");
    assert_eq!(row_state_class(seen), "seen");
    assert_eq!(row_state_class(read), "read");
    // article suppresses the seen-dim → no dim class (check still rendered separately)
    assert_eq!(row_state_class(article), "");
    assert_eq!(row_state_class(both), "read");
}
```

- [ ] **Step 2: Run, verify fail.** Run: `cargo test -p chaos-ui should_record_app_open merge_flags row_state_class -v`. Expected: FAIL.

- [ ] **Step 3: Implement the pure helpers** in `analytics.rs`:

```rust
use chaos_domain::ViewFlags;

/// True if an `app_open` should be logged: never logged, or the last was > 5 min ago.
pub(crate) fn should_record_app_open(last_secs: Option<i64>, now_secs: i64) -> bool {
    match last_secs {
        None => true,
        Some(last) => now_secs - last >= 300,
    }
}

pub(crate) fn merge_flags(a: ViewFlags, b: ViewFlags) -> ViewFlags {
    ViewFlags { seen: a.seen || b.seen, comments: a.comments || b.comments, article: a.article || b.article }
}

/// The dim class for a row. Dimming is the reading axis only; opening the
/// article suppresses the seen-dim (its check is the signal).
pub(crate) fn row_state_class(f: ViewFlags) -> &'static str {
    if f.comments {
        "read"
    } else if f.seen && !f.article {
        "seen"
    } else {
        ""
    }
}
```

- [ ] **Step 4: Run tests + wasm check.** Expected: green.

- [ ] **Step 5: Commit** `feat(ui): pure analytics helpers (throttle, merge, row class)`.

---

### Task B2: Outbox + overlay + flush

**Files:**
- Modify: `crates/chaos-ui/src/analytics.rs`

- [ ] **Step 1: Implement the outbox + overlay + flush** (browser glue; the pure
parts are already tested in B1). Use `crate::offline::cache_get`/`cache_put`
patterns for localStorage (or `crate::local_storage()` directly).

```rust
use std::collections::HashMap;
use chaos_domain::{
    EventItem, RecordEventsRequest, RecordViewsRequest, Source, ViewEvent, ViewEventItem, ViewFlags,
};
use leptos::prelude::*;

const OUTBOX_KEY: &str = "chaos-view-outbox";
const EVENTS_KEY: &str = "chaos-event-outbox";
const APPOPEN_KEY: &str = "chaos-appopen-at";

/// Global optimistic overlay: (source-as-str, post_id) -> flags. Provided once
/// in App context so NewsPage rows and the reader share it.
#[derive(Clone, Copy)]
pub(crate) struct Overlay(pub RwSignal<HashMap<(String, String), ViewFlags>>);

pub(crate) fn provide_overlay() {
    provide_context(Overlay(RwSignal::new(HashMap::new())));
}
pub(crate) fn overlay() -> Overlay {
    use_context::<Overlay>().expect("Overlay provided by App")
}

fn read_vec<T: serde::de::DeserializeOwned>(key: &str) -> Vec<T> {
    crate::local_storage()
        .and_then(|s| s.get_item(key).ok().flatten())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}
fn write_vec<T: serde::Serialize>(key: &str, v: &[T]) {
    if let Some(s) = crate::local_storage() {
        let _ = s.set_item(key, &serde_json::to_string(v).unwrap_or_default());
    }
}

/// Record a per-post event: OR into the overlay (instant restyle), queue it,
/// schedule a flush. Deduped: if the overlay already has this flag, skip the queue.
pub(crate) fn record_view(source: Source, post_id: &str, event: ViewEvent) {
    let flag = event_flag(event);
    let key = (source.as_str().to_string(), post_id.to_string());
    let ov = overlay().0;
    let already = ov.with_untracked(|m| m.get(&key).copied().unwrap_or_default());
    // OR the new flag in.
    let mut next = already;
    match event {
        ViewEvent::Seen => next.seen = true,
        ViewEvent::OpenedComments => { next.comments = true; next.seen = true; }
        ViewEvent::OpenedArticle => { next.article = true; next.seen = true; }
    }
    if next == already {
        return; // nothing new to record
    }
    ov.update(|m| { m.insert(key, next); });
    let mut q: Vec<ViewEventItem> = read_vec(OUTBOX_KEY);
    q.push(ViewEventItem { source, post_id: post_id.to_string(), event, at: now_utc() });
    write_vec(OUTBOX_KEY, &q);
    schedule_flush();
    let _ = flag;
}

pub(crate) fn record_event(kind: &str, detail: Option<String>) {
    let mut q: Vec<EventItem> = read_vec(EVENTS_KEY);
    q.push(EventItem { kind: kind.to_string(), detail, at: now_utc() });
    write_vec(EVENTS_KEY, &q);
    schedule_flush();
}

/// Merge the server map into the overlay (never clears locally-pending flags).
pub(crate) fn merge_server_map(source: Source, map: chaos_domain::ViewedMap) {
    let ov = overlay().0;
    ov.update(|m| {
        for (id, f) in map {
            let key = (source.as_str().to_string(), id);
            let cur = m.get(&key).copied().unwrap_or_default();
            m.insert(key, super::analytics::merge_flags(cur, f));
        }
    });
}

/// Flush both outboxes when Online; clear on success. Called debounced + on reconnect.
pub(crate) async fn flush(client: crate::ChaosClientHandle /* match the real client type */) {
    // views
    let views: Vec<ViewEventItem> = read_vec(OUTBOX_KEY);
    if !views.is_empty()
        && client.record_views(&RecordViewsRequest { events: views.clone() }).await.is_ok()
    {
        write_vec::<ViewEventItem>(OUTBOX_KEY, &[]);
    }
    let events: Vec<EventItem> = read_vec(EVENTS_KEY);
    if !events.is_empty()
        && client.record_events(&RecordEventsRequest { events: events.clone() }).await.is_ok()
    {
        write_vec::<EventItem>(EVENTS_KEY, &[]);
    }
}
```

> Fill in the real client handle type (from `use_client()` — likely
> `ChaosClient`), `now_utc()` (`chrono::Utc::now()` — allowed in browser wasm),
> `event_flag`, and `schedule_flush()` (a debounced `spawn_local` guarded by a
> `thread_local` timer/flag: on call, (re)start a ~1.5s timeout via
> `set_timeout`, then `spawn_local(flush(client))` — grab the client + conn via
> context inside; only actually POST when `conn == Online`, else leave queued).
> Keep DOM/timer glue here; the pure logic stays in B1.

- [ ] **Step 2: Run wasm check + clippy.** (No new unit tests here — logic is
covered by B1; this is browser glue.) `cargo check -p chaos-ui --target wasm32-unknown-unknown && cargo clippy -p chaos-ui --all-targets -- -D warnings`. Expected: green.

- [ ] **Step 3: Commit** `feat(ui): analytics outbox, overlay, flush`.

---

### Task B3: App boot wiring (overlay + app_open + reconnect flush)

**Files:**
- Modify: `crates/chaos-ui/src/lib.rs`
- Modify: `crates/chaos-ui/src/offline.rs` (reconnect hook)

- [ ] **Step 1: Provide the overlay + record app_open.** In `App`, call
`crate::analytics::provide_overlay()` once. After the session is confirmed
(the `me()` success / cached-user restore path, `lib.rs` ~495-542), when a
session exists, call an `analytics::maybe_record_app_open()` that reads/writes
`APPOPEN_KEY` via `should_record_app_open` and, when due, `record_event("app_open", None)`.

```rust
// analytics.rs
pub(crate) fn maybe_record_app_open() {
    let now = (js_sys::Date::now() / 1000.0) as i64;
    let last = crate::local_storage()
        .and_then(|s| s.get_item(APPOPEN_KEY).ok().flatten())
        .and_then(|v| v.parse::<i64>().ok());
    if should_record_app_open(last, now) {
        if let Some(s) = crate::local_storage() {
            let _ = s.set_item(APPOPEN_KEY, &now.to_string());
        }
        record_event("app_open", None);
    }
}
```

- [ ] **Step 2: Reconnect flush.** In `offline::probe`, on the Offline→Online
transition (where `conn.set(Connectivity::Online)` happens after having been
non-Online), trigger a flush: `crate::analytics::flush_now()` (a sync wrapper
that `spawn_local`s `flush(client)` using the client from context). Keep it
best-effort.

- [ ] **Step 3: wasm check + clippy + fmt.** Expected: green.

- [ ] **Step 4: Commit** `feat(ui): provide overlay, record app_open, flush on reconnect`.

---

### Task B4: `ViewedState` context + `post_row_view` rendering

**Files:**
- Modify: `crates/chaos-ui/src/pages/dashboard.rs` (`post_row_view`)
- Modify: `crates/chaos-web/styles.css`

- [ ] **Step 1: Define `ViewedState`** (in `dashboard.rs` or `analytics.rs`):

```rust
/// Present only when authed on /news + reader. Absence → plain rows.
#[derive(Clone, Copy)]
pub(crate) struct ViewedState { pub source: Source }
```

(The overlay itself is the global `Overlay` context; `ViewedState` just signals
"tracking is on for this source". `post_row_view` reads both.)

- [ ] **Step 2: Render state in `post_row_view`.** Read
`use_context::<ViewedState>()`. When present and `item.id` is `Some(id)`:
compute `flags` reactively from the overlay for `(source, id)`; add
`data-view-id="{source}:{id}"` to the `<li>`; add the dim class via
`row_state_class(flags)`; render the article check before the favicon when
`flags.article`; and add `on:click` recorders to the title and favicon.

```rust
let tracked = use_context::<ViewedState>().is_some();
let vid = item.id.clone();
// reactive flags for this row
let ov = tracked.then(|| crate::analytics::overlay().0);
let flags = {
    let (src, id) = (source, vid.clone());
    move || match (ov, &id) {
        (Some(ov), Some(id)) => ov.with(|m| m.get(&(src.as_str().to_string(), id.clone())).copied().unwrap_or_default()),
        _ => chaos_domain::ViewFlags::default(),
    }
};
// class + check are reactive:
//   <li class=move || format!("post-row {}", crate::analytics::row_state_class(flags()))
//       data-view-id=... >
// article check inside .post-meta-right, before the favicon:
//   {move || flags().article.then(|| view!{ <span class="article-check">"✓"</span> })}
```

Wire recorders (only when `tracked` && id present):
- title `<A>` — `on:click=move |_| crate::analytics::record_view(source, &id, ViewEvent::OpenedComments)`.
- favicon `<a>` — `on:click=move |_| crate::analytics::record_view(source, &id, ViewEvent::OpenedArticle)`.

> Preserve the existing markup (right-cluster `.post-meta-right`: domain, then
> the new check, then favicon). When `!tracked`, render exactly as today (no
> data attr, no class, no check, no recorders) — the desktop widget and
> logged-off web are unaffected.

- [ ] **Step 3: CSS** (`styles.css`, near `.post-domain`):

```css
li.post-row.seen .post-title { opacity: 0.72; }
li.post-row.read .post-title { opacity: 0.52; }
.article-check { color: var(--accent); opacity: 0.85; font-size: 0.8rem; font-weight: 700; }
```

- [ ] **Step 4: Run tests + wasm + clippy + fmt.** (`row_state_class` unit test
from B1 covers the class logic.) Expected: green.

- [ ] **Step 5: Commit** `feat(news): render viewed states + record comment/article opens`.

---

### Task B5: NewsPage — load map, provide context, IntersectionObserver

**Files:**
- Modify: `crates/chaos-ui/src/pages/news.rs`

- [ ] **Step 1: Provide `ViewedState` + load the map when authed.** In
`NewsPage`, if `crate::use_session().0.get_untracked().is_some()`, provide
`ViewedState { source: source.get() }` (re-provide/refresh on source change) and,
in a resource/effect keyed on `source`, call
`analytics::merge_server_map(source, client.viewed_map(source).await?)`.

> `ViewedState` carries the *current* source; since it changes with the source
> signal, provide it inside the reactive scope or store the source in the
> context as a signal. Simplest: make `ViewedState { source: Source }` provided
> fresh whenever source changes — or read `source` from a signal. Pick the
> cleaner option so `post_row_view` always sees the active source.

- [ ] **Step 2: IntersectionObserver for "seen".** After the list renders,
observe each `li.post-row[data-view-id]`. On an entry with
`intersectionRatio >= 0.5`, parse `data-view-id` (`"{source}:{id}"`) and
`analytics::record_view(source, id, ViewEvent::Seen)`. Recreate/rebind the
observer when the shown list changes (Effect keyed on the list): disconnect the
old observer, create a new `web_sys::IntersectionObserver` with a threshold of
`0.5`, and observe every current row node (query the DOM for
`.post-row[data-view-id]` after render, or collect NodeRefs).

```rust
// sketch — browser glue:
use wasm_bindgen::prelude::*;
let cb = Closure::<dyn Fn(Vec<web_sys::IntersectionObserverEntry>)>::new(
    move |entries: Vec<web_sys::IntersectionObserverEntry>| {
        for e in entries {
            if e.intersection_ratio() >= 0.5 {
                if let Some(el) = e.target().dyn_ref::<web_sys::HtmlElement>() {
                    if let Some(vid) = el.dataset().get("viewId") {
                        if let Some((src, id)) = vid.split_once(':') {
                            if let Some(s) = Source::from_str(src) {
                                crate::analytics::record_view(s, id, ViewEvent::Seen);
                            }
                        }
                    }
                }
            }
        }
    },
);
// build IntersectionObserver { threshold: [0.5] }, observe each row, keep cb alive.
```

> Keep the `Closure` alive (store it) so the callback isn't dropped. Observe
> only when authed (`ViewedState` present). This is browser-only; verify in the
> headless-browser run, not a unit test.

- [ ] **Step 3: wasm check + clippy + fmt + tests.** Expected: green.

- [ ] **Step 4: Commit** `feat(news): load viewed-map, provide context, observe rows for 'seen'`.

---

### Task B6: Reader wiring

**Files:**
- Modify: `crates/chaos-ui/src/pages/reader.rs`

- [ ] **Step 1: Record on reader load (authed).** When the thread loads and a
session exists, `analytics::record_view(source, &id, ViewEvent::OpenedComments)`
and `analytics::record_event("reader_open", Some(format!("{}:{}", source.as_str(), id)))`.
Do this once per load (e.g., in an Effect that fires when the thread resolves,
guarded so it records once).

- [ ] **Step 2: Article link → opened_article.** On the reader's story-title
link (to the external article), add
`on:click=move |_| analytics::record_view(source, &id, ViewEvent::OpenedArticle)`
(only when authed).

- [ ] **Step 3: wasm check + clippy + fmt.** Expected: green.

- [ ] **Step 4: Commit** `feat(reader): record opened-comments, reader_open, article open`.

---

### Task B7: End-to-end verification + polish

**Files:**
- (verification only; fixes as needed)

- [ ] **Step 1: Build the web bundle + run against a local server** (server built
from Plan A). Headless-browser check (per the session's verify-in-browser
practice): (a) scrolling marks rows `seen` (dim); (b) tapping a title dims it
`read` and adds no check; (c) tapping a favicon adds the `✓` between domain and
favicon WITHOUT dimming; (d) reloading `/news` keeps the states (server round-trip);
(e) logged-off shows plain rows.

- [ ] **Step 2: Full verification.** `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check && cargo check -p chaos-ui --target wasm32-unknown-unknown` and a release build of `chaos-server`/`chaos-desktop` + trunk web bundle.

- [ ] **Step 3: Docs.** Update `docs/HANDOFF.md` + `docs/ROADMAP.md` with the
viewed-state + analytics feature (new tables, endpoints, events). Commit.

- [ ] **Step 4: Commit** any polish `feat(news): viewed-state end-to-end verified`.

---

## Self-review notes
- Spec coverage: B1 pure helpers (throttle/merge/class); B2 outbox+overlay+flush;
  B3 app boot (overlay, app_open, reconnect); B4 row rendering + comment/article
  recorders; B5 NewsPage map-load + context + IntersectionObserver ('seen'); B6
  reader (opened-comments, reader_open, article); B7 verify + docs. Covers §Plan B.
- Type consistency: `record_view(Source, &str, ViewEvent)`, `record_event(&str, Option<String>)`,
  `merge_server_map(Source, ViewedMap)`, `Overlay(RwSignal<HashMap<(String,String),ViewFlags>>)`,
  `row_state_class(ViewFlags) -> &'static str`, `should_record_app_open(Option<i64>, i64)`,
  `ViewedState { source: Source }` — consistent across B1-B6.
- Depends on Plan A's `viewed_map`/`record_views`/`record_events` client calls
  and the domain types.
- Scope: only `/news` + reader render/record (via `ViewedState` presence);
  desktop widget + logged-off unaffected.
- Browser-only glue (observer, timers, localStorage) is verified in B7's
  headless run; pure logic is unit-tested in B1.
