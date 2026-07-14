# Offline Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A connectivity signal + cache-first read layer so every chaos client keeps rendering last-known-good data when the chaos server is unreachable, stops polling it, and recovers via an offline badge.

**Architecture:** Port yomu's offline core (`/projects/rust/yomu/crates/yomu-ui/src/offline.rs`, refined in yomu 1.10.0): an app-wide `Connectivity` signal decided by the health probe alone, a `cached()` helper over localStorage that serves stale data on failure and downgrades connectivity on transport errors, request deadlines in `chaos-client`, and per-page offline states (services unpolled + unknown, links read-only, mutations disabled). Spec: `docs/superpowers/specs/2026-07-14-offline-ux-design.md`.

**Tech Stack:** Rust, Leptos 0.8 CSR (chaos-ui), reqwest dual-target (chaos-client), localStorage via web-sys, serde_json.

**Conventions for every task:** Commit with trailers
```
Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
```
and **`git -c commit.gpgsign=false commit …`** (the signing YubiKey is unavailable in this session). Never push. Run `cargo fmt --all` before each commit. `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` must pass at every commit. The uuid crate has no v4/v7 features — tests use `Uuid::from_u128(n)`/`Uuid::nil()`. IDE diagnostics shown mid-task are often stale — trust `cargo` output only.

---

### Task 1: Request deadlines in chaos-client

**Files:**
- Modify: `crates/chaos-client/src/lib.rs`

An unreachable host must fail in seconds, not minutes. Mirror yomu (`/projects/rust/yomu/crates/yomu-client/src/lib.rs:30-32` and its `check_status`): reqwest supports per-`Request` timeouts on both native and wasm via `timeout_mut()`.

- [ ] **Step 1: Add the constants and default-timeout plumbing**

At the top of `crates/chaos-client/src/lib.rs` (near the existing consts/imports):

```rust
use std::time::Duration;

/// Deadline for regular data requests. Generous for a LAN server; short
/// enough that an unreachable host fails the page fast instead of hanging.
const DATA_TIMEOUT: Duration = Duration::from_secs(8);
/// The health probe decides connectivity; it must answer (or fail) fast.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(3);
```

In the private request pipeline (`check_status`, where the bearer token is applied — read the current body first), after building the `Request` and before executing it, set the default deadline exactly like yomu does:

```rust
// Every request gets a deadline; an unreachable server must fail fast,
// not hang "Loading" for minutes. Callers that set their own (health)
// keep it.
if request.timeout().is_none() {
    *request.timeout_mut() = Some(DATA_TIMEOUT);
}
```

If chaos-client's pipeline builds `RequestBuilder`s and calls `.send()` directly instead of materializing a `Request`, restructure it the way yomu's `check_status` does (`RequestBuilder::build()` then `Client::execute`).

- [ ] **Step 2: Give `health()` the short deadline**

```rust
pub async fn health(&self) -> Result<HealthResponse> {
    let req = self
        .http
        .get(self.url("api/v1/health")?)
        .timeout(HEALTH_TIMEOUT);
    self.send(req).await
}
```

(If `RequestBuilder::timeout` is unavailable on wasm in the pinned reqwest version, thread a `timeout: Duration` parameter through `send`/`check_status` instead — check how yomu solved it and match it.)

- [ ] **Step 3: Build both targets, test, commit**

Run: `cargo test -p chaos-client && cargo build -p chaos-client --target wasm32-unknown-unknown && cargo clippy --workspace --all-targets -- -D warnings`
Expected: green.

```bash
git add crates/chaos-client && git -c commit.gpgsign=false commit -m "feat(client): 8s data / 3s health request deadlines"
```
(with the standard trailers)

---

### Task 2: offline.rs — connectivity signal, cache store, cached() helper

**Files:**
- Create: `crates/chaos-ui/src/offline.rs`
- Modify: `crates/chaos-ui/src/lib.rs` (add `pub(crate) mod offline;` to the module list)
- Modify: `crates/chaos-ui/Cargo.toml` (add `serde_json` and `serde` if not already deps; check first — chaos-domain re-exports may suffice, but `serde_json` is needed)

- [ ] **Step 1: Write the module with pure logic separated for native tests**

Create `crates/chaos-ui/src/offline.rs`:

```rust
//! Offline support: the app-wide connectivity state and the cache-first
//! read path. Modeled on yomu's offline core.
//!
//! Connectivity is decided by the health probe ALONE — never
//! `navigator.onLine` (a device away from a self-hosted server has
//! connectivity but no route home), and never a per-request success
//! (only the probe promotes to Online). The first failed server request
//! downgrades Online → Offline.

use chaos_client::{ChaosClient, ClientError};
use leptos::prelude::*;
use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Connectivity {
    /// Boot: the first health probe hasn't answered yet.
    Checking,
    Online,
    Offline,
}

pub(crate) fn use_connectivity() -> RwSignal<Connectivity> {
    use_context::<RwSignal<Connectivity>>().expect("Connectivity provided by App")
}

const CACHE_PREFIX: &str = "chaos-cache:";
const SERVERS_SEEN_KEY: &str = "chaos-servers-seen";

pub(crate) fn cache_put<T: Serialize>(key: &str, value: &T) {
    if let (Some(storage), Ok(json)) = (crate::local_storage(), serde_json::to_string(value)) {
        let _ = storage.set_item(&format!("{CACHE_PREFIX}{key}"), &json);
    }
}

pub(crate) fn cache_get<T: DeserializeOwned>(key: &str) -> Option<T> {
    let raw = crate::local_storage()?
        .get_item(&format!("{CACHE_PREFIX}{key}"))
        .ok()??;
    serde_json::from_str(&raw).ok()
}

/// The one cache-first read path. Offline (or still checking) with a cached
/// copy: serve it immediately, zero network. Online: fetch; a success
/// overwrites the cache (that's the only invalidation — no TTL); a
/// *transport* failure downgrades connectivity and falls back to the cache.
/// API errors (401, 404, validation) pass through untouched: the server
/// answered, so this is not a connectivity problem and stale data would be
/// wrong.
///
/// Returns `(value, stale)` — `stale` means "came from the cache".
pub(crate) async fn cached<T, Fut>(
    conn: RwSignal<Connectivity>,
    key: &str,
    fetch: Fut,
) -> Result<(T, bool), ClientError>
where
    T: Serialize + DeserializeOwned,
    Fut: Future<Output = Result<T, ClientError>>,
{
    if conn.get_untracked() != Connectivity::Online
        && let Some(hit) = cache_get::<T>(key)
    {
        return Ok((hit, true));
    }
    match fetch.await {
        Ok(value) => {
            cache_put(key, &value);
            Ok((value, false))
        }
        Err(err) if is_connectivity_error(&err) => {
            // Downgrade only — promotion to Online is the probe's job.
            if conn.get_untracked() == Connectivity::Online {
                conn.set(Connectivity::Offline);
            }
            match cache_get::<T>(key) {
                Some(hit) => Ok((hit, true)),
                None => Err(err),
            }
        }
        Err(err) => Err(err),
    }
}

/// Only failures to REACH the server say anything about connectivity.
fn is_connectivity_error(err: &ClientError) -> bool {
    matches!(err, ClientError::Transport(_))
}

// ---- "server seen" gate memory ----
// Distinguishes "misconfigured / never reached" (connect form) from "known
// server that is just offline right now" (cached UI + badge).

fn seen_list(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

pub(crate) fn server_seen(base: &str) -> bool {
    crate::local_storage()
        .and_then(|s| s.get_item(SERVERS_SEEN_KEY).ok().flatten())
        .is_some_and(|raw| seen_list(&raw).iter().any(|b| b == base))
}

pub(crate) fn mark_server_seen(base: &str) {
    let Some(storage) = crate::local_storage() else {
        return;
    };
    let raw = storage.get_item(SERVERS_SEEN_KEY).ok().flatten().unwrap_or_default();
    let mut list = seen_list(&raw);
    if !list.iter().any(|b| b == base) {
        list.push(base.to_string());
        let _ = storage.set_item(SERVERS_SEEN_KEY, &list.join("\n"));
    }
}

/// One bounded health probe; the only code path that can set `Online`.
/// Returns whether the server answered.
pub(crate) async fn probe(client: &ChaosClient, conn: RwSignal<Connectivity>) -> bool {
    match client.health().await {
        Ok(health) => {
            crate::set_server_fahrenheit(health.fahrenheit);
            mark_server_seen(client.base().as_str());
            conn.set(Connectivity::Online);
            true
        }
        Err(_) => {
            conn.set(Connectivity::Offline);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_errors_are_connectivity_errors_api_errors_are_not() {
        assert!(is_connectivity_error(&ClientError::Transport(
            "connection refused".into()
        )));
        assert!(!is_connectivity_error(&ClientError::Api {
            status: 401,
            message: "who are you".into()
        }));
        assert!(!is_connectivity_error(&ClientError::Decode("bad json".into())));
    }

    #[test]
    fn seen_list_parses_and_ignores_blanks() {
        let raw = "http://zeus:4600/\n\n  http://other:4600/  \n";
        assert_eq!(seen_list(raw), ["http://zeus:4600/", "http://other:4600/"]);
        assert!(seen_list("").is_empty());
    }
}
```

Notes for the implementer:
- `HealthResponse::fahrenheit` — check the actual field (the gate currently does `set_server_fahrenheit(health.fahrenheit)` in `lib.rs:613`); match it.
- `crate::local_storage()` exists at `lib.rs:114`; it is private — change it to `pub(crate)`.
- `cached()` and probe run under wasm; `cargo test` (native) only exercises the pure helpers — that is expected. `cache_get`/`cache_put` return `None`/no-op natively because `web_sys::window()` is `None`.

- [ ] **Step 2: Run tests to verify**

Run: `cargo test -p chaos-ui offline && cargo build -p chaos-ui --target wasm32-unknown-unknown`
Expected: 2 new tests PASS, wasm builds.

- [ ] **Step 3: Commit**

```bash
git add crates/chaos-ui && git -c commit.gpgsign=false commit -m "feat(ui): offline core — connectivity signal, cache store, cached() helper"
```

---

### Task 3: Wire connectivity into App + ServerGate; add the OfflineBadge

**Files:**
- Modify: `crates/chaos-ui/src/lib.rs` (App L422, ServerGate L604)

- [ ] **Step 1: Provide the signal and the online-event probe in `App`**

In `App` (`lib.rs:422`), right after `provide_context(SharedClient(...))`:

```rust
// Offline support: one app-wide connectivity signal, Checking until the
// gate's first probe answers. The browser's `online` event buys one free
// re-probe (it only says "some network came back", not "the server is
// reachable", so it triggers a probe rather than trusting it).
let conn = RwSignal::new(offline::Connectivity::Checking);
provide_context(conn);
let client_for_online = use_client();
let online_probe = window_event_listener(leptos::ev::online, move |_| {
    let client = client_for_online.clone();
    spawn_local(async move {
        offline::probe(&client, conn).await;
    });
});
on_cleanup(move || online_probe.remove());
```

(`use_client()` requires `AppConfig` in context — it is provided just above; keep this block after `provide_context(config)`.)

- [ ] **Step 2: Rewrite `ServerGate`'s probe outcome handling**

Replace the `spawn_local` block in `ServerGate` (`lib.rs:608-618`) with:

```rust
let conn = offline::use_connectivity();
let seen = offline::server_seen(use_client().base().as_str());
spawn_local(async move {
    if offline::probe(&client, conn).await {
        gate.set(GateState::Ready);
    } else if seen {
        // A server we've reached before is just offline right now: boot
        // into the cached UI with the badge instead of the connect form.
        gate.set(GateState::Ready);
    } else {
        gate.set(GateState::Unreachable);
    }
});
```

`probe` already handles `set_server_fahrenheit` and `mark_server_seen`. The "Continue anyway" button (`lib.rs:651`) must also leave connectivity Offline — `probe` already set it, so `gate.set(GateState::Ready)` alone remains correct.

- [ ] **Step 3: Add the `OfflineBadge` component and mount it**

In `offline.rs` add (modeled on yomu's `OfflineBadge`, `yomu-ui/src/lib.rs:204`):

```rust
/// Fixed badge shown whenever the app is not Online; clicking it re-probes.
/// This is the ONLY reconnect path besides the browser `online` event — no
/// background timers.
#[component]
pub(crate) fn OfflineBadge() -> impl IntoView {
    let conn = use_connectivity();
    let busy = RwSignal::new(false);
    let failed_flash = RwSignal::new(false);

    let retry = move |_| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        let client = crate::use_client();
        leptos::task::spawn_local(async move {
            if !probe(&client, conn).await {
                failed_flash.set(true);
                leptos::prelude::set_timeout(
                    move || failed_flash.set(false),
                    std::time::Duration::from_millis(1800),
                );
            }
            busy.set(false);
        });
    };

    view! {
        {move || (conn.get() != Connectivity::Online).then(|| {
            let label = move || {
                if busy.get() {
                    "connecting…"
                } else if failed_flash.get() {
                    "still offline"
                } else {
                    "offline — retry"
                }
            };
            view! {
                <button class="offline-badge" on:click=retry>
                    {label}
                </button>
            }
        })}
    }
}
```

Mount it in `App`'s view right after `<search::QuickSearch open=search_open/>`:

```rust
<offline::OfflineBadge/>
```

- [ ] **Step 4: Style the badge**

Find the stylesheet (`crates/chaos-web/styles.css` or wherever `.server-gate` is styled — grep for `server-gate`). Add:

```css
.offline-badge {
    position: fixed;
    right: 0.75rem;
    bottom: 3.5rem; /* above the mobile tabbar */
    z-index: 60;
    padding: 0.35rem 0.7rem;
    border-radius: 999px;
    border: 1px solid var(--border, #444);
    background: var(--surface, #222);
    color: var(--danger, #e05555);
    font-size: 0.8rem;
    cursor: pointer;
}
@media (min-width: 800px) {
    .offline-badge { bottom: 0.75rem; }
}
```

Match existing CSS variable names used by the current theme files (grep for `--danger`/similar and reuse the real ones).

- [ ] **Step 5: Build, test, commit**

Run: `cargo test --workspace && cargo build -p chaos-ui --target wasm32-unknown-unknown && cargo clippy --workspace --all-targets -- -D warnings`

```bash
git add -A && git -c commit.gpgsign=false commit -m "feat(ui): connectivity wired into App/ServerGate; offline badge retry"
```

---

### Task 4: Offline-aware polling hook

**Files:**
- Modify: `crates/chaos-ui/src/hooks.rs`

- [ ] **Step 1: Gate `use_polled_resource` on connectivity**

Replace the body of `use_polled_resource` (`hooks.rs:63-84`) with a version that only tracks the tick/refresh sources while Online, plus an opt-in for widgets that can fetch without the server (weather, HN/lobsters — used in the direct-fetch plan):

```rust
pub(crate) fn use_polled_resource<T, Fut>(
    interval: Duration,
    version: Option<RwSignal<u32>>,
    fetch: impl Fn() -> Fut + 'static,
) -> LocalResource<T>
where
    T: 'static,
    Fut: Future<Output = T> + 'static,
{
    use_polled_resource_with(interval, version, false, fetch)
}

/// Like [`use_polled_resource`], but `poll_offline` keeps the interval
/// refetching while the server is unreachable — for resources that can
/// fetch their data without the server (direct Open-Meteo / HN).
pub(crate) fn use_polled_resource_with<T, Fut>(
    interval: Duration,
    version: Option<RwSignal<u32>>,
    poll_offline: bool,
    fetch: impl Fn() -> Fut + 'static,
) -> LocalResource<T>
where
    T: 'static,
    Fut: Future<Output = T> + 'static,
{
    let tick = use_interval_tick(interval);
    let refresh = use_context::<RefreshTick>();
    let conn = crate::offline::use_connectivity();
    LocalResource::new(move || {
        // While offline, interval ticks and manual refreshes must not fire
        // requests at an unreachable server ("not probing when offline").
        // Reading `conn` tracks it, so recovery re-runs every resource once.
        if poll_offline || conn.get() == crate::offline::Connectivity::Online {
            tick.track();
            if let Some(RefreshTick(refresh)) = refresh {
                refresh.track();
            }
        } else {
            conn.track();
        }
        if let Some(version) = version {
            version.track();
        }
        fetch()
    })
}
```

Note: `conn.get()` already tracks; the `else { conn.track() }` is for symmetry — simplify to a single `conn.get()` read at the top:

```rust
let online = conn.get() == crate::offline::Connectivity::Online;
if poll_offline || online { tick.track(); …refresh… }
```

Components rendered outside `App` (unit tests) have no Connectivity context — `use_connectivity()` would panic. Make the hook total: use `use_context::<RwSignal<Connectivity>>()` directly and treat `None` as Online:

```rust
let online = use_context::<RwSignal<crate::offline::Connectivity>>()
    .map(|c| c.get() == crate::offline::Connectivity::Online)
    .unwrap_or(true);
```

- [ ] **Step 2: Verify workspace still green, commit**

Run: `cargo test --workspace && cargo build -p chaos-ui --target wasm32-unknown-unknown && cargo clippy --workspace --all-targets -- -D warnings`

```bash
git add crates/chaos-ui && git -c commit.gpgsign=false commit -m "feat(ui): polled resources pause while offline, refetch on recovery"
```

---

### Task 5: Dashboard offline — cached layout, unpolled services, cached widgets

**Files:**
- Modify: `crates/chaos-ui/src/pages/dashboard.rs`
- Modify: `crates/chaos-ui/src/components.rs` (ServiceGrid/ServiceCard get a `read_only` prop)

- [ ] **Step 1: Cache the layout**

In `Dashboard` (`dashboard.rs:27-33`):

```rust
let conn = crate::offline::use_connectivity();
let layout = LocalResource::new({
    let client = client.clone();
    move || {
        conn.track(); // recovery re-fetches the layout once
        let client = client.clone();
        async move {
            crate::offline::cached(conn, "dashboard", client.dashboard())
                .await
                .map(|(layout, _)| layout)
        }
    }
});
```

(The stale flag is dropped here — the badge already tells the user; the layout has no per-widget staleness UI.)

- [ ] **Step 2: Services widget — cache + unknown state + no controls offline**

In `ServicesWidget` (`dashboard.rs:99`):

```rust
let conn = crate::offline::use_connectivity();
let services = crate::hooks::use_polled_resource(SERVICES_REFRESH, Some(action.version), {
    let client = client.clone();
    move || {
        let client = client.clone();
        async move { crate::offline::cached(conn, "services", client.services()).await }
    }
});
let services = Memo::new(move |_| {
    services
        .get()
        .map(|r| r.map_err(|e| e.to_string()))
});
```

In the `Some(Ok((list, stale)))` arm: when `stale`, the statuses are from another era — force them honest before rendering:

```rust
Some(Ok((mut list, stale))) => {
    if stale {
        for service in &mut list {
            service.status.state = HealthState::Unknown;
            service.status.latency_ms = None;
        }
    }
    let count = list.len();
    view! {
        <Collapsible count collapsed>
            <ServiceGrid services=list controls=(action.busy, run) read_only=stale/>
        </Collapsible>
    }
    .into_any()
}
```

Import `chaos_domain::HealthState` if not already imported.

- [ ] **Step 3: `read_only` prop on ServiceGrid/ServiceCard**

In `components.rs`, add `#[prop(optional)] read_only: bool` to `ServiceGrid`, pass it to `ServiceCard`, and in `ServiceCard` change the action-button condition (`components.rs:177`):

```rust
let action = (service.def.unit.is_some() && !read_only).then(|| { … });
```

The tile stays a link (the target may be reachable even when the chaos server isn't — e.g. public services).

- [ ] **Step 4: DataWidget + SystemdWidget through the cache**

In `DataWidget` (`dashboard.rs:194-202`):

```rust
let conn = crate::offline::use_connectivity();
let data = crate::hooks::use_polled_resource(WIDGET_REFRESH, None, {
    let client = client.clone();
    move || {
        let client = client.clone();
        let id = id.clone();
        let location = weather_location.clone().flatten();
        async move {
            crate::offline::cached(conn, &format!("widget:{id}"), async {
                client.widget_data(&id, location.as_deref()).await
            })
            .await
        }
    }
});
let data = Memo::new(move |_| data.get().map(|r| r.map_err(|e| e.to_string())));
```

Adjust the match arms from `Some(Ok(WidgetData::…))` to `Some(Ok((WidgetData::…, stale)))` patterns — destructure the tuple once at the top:

```rust
Some(Ok((data, stale))) => { /* existing per-kind match on data; add a
    stale hint next to the <h2> when stale */ }
```

Add the stale hint in the header:

```rust
<h2>{title} {move || /* when stale */ view! { <span class="muted stale-hint">"· cached"</span> }}</h2>
```

Implementation freedom: a `stale` RwSignal written by the resource arm or restructuring the view — keep it simple; the acceptance criterion is a small "· cached" marker on widgets rendering cached data, and identical rendering otherwise.

`SystemdWidget` (`dashboard.rs:303`): keep `use_polled_resource` (pauses offline). Wrap in `cached(conn, &format!("widget:{id}"), …)` too, and when the payload is stale render the unit rows with action buttons hidden (same pattern as services: gate the buttons on `!stale`).

- [ ] **Step 5: Test, commit**

Run: `cargo test --workspace && cargo build -p chaos-ui --target wasm32-unknown-unknown && cargo clippy --workspace --all-targets -- -D warnings`

```bash
git add -A && git -c commit.gpgsign=false commit -m "feat(ui): dashboard renders cached layout/services/widgets offline"
```

---

### Task 6: Links page offline — read-only cached default view

**Files:**
- Modify: `crates/chaos-ui/src/pages/links.rs`

- [ ] **Step 1: Cache the three resources on the default query**

The default view is: no collection, no tag, no search, page 0. Add a helper near `effective_query`:

```rust
/// Only the default view is cached for offline browsing: per-query caching
/// would explode the key space for no real offline value.
fn is_default_query(query: &LinkQuery) -> bool {
    query.collection.is_none()
        && query.tag.is_none()
        && query.q.is_none()
        && query.offset.unwrap_or(0) == 0
}
```

(Match the real `LinkQuery` field names — check `chaos-domain`; adjust `q`/`search` etc. accordingly.)

Wrap the resources (`links.rs:86-110`): links go through `cached` only when the query is default; collections and tags always:

```rust
let conn = crate::offline::use_connectivity();
let links = LocalResource::new({
    let client = client.clone();
    move || {
        refresh.track();
        conn.track();
        let query = query.get();
        let client = client.clone();
        async move {
            if is_default_query(&query) {
                crate::offline::cached(conn, "links", client.list_links(&query))
                    .await
                    .map(|(page, _)| page)
            } else if conn.get_untracked() != crate::offline::Connectivity::Online {
                // Filtered/paged views are not cached; don't hit the network.
                Err(chaos_client::ClientError::Transport(
                    "unavailable offline".into(),
                ))
            } else {
                client.list_links(&query).await
            }
        }
    }
});
```

`collections` and `tags`: same wrapping with keys `"collections"` / `"tags"`, dropping the stale flag.

- [ ] **Step 2: Offline read-only UI**

Where the toolbar/quick-add renders (`links.rs:120-130`), gate the mutating affordances:

```rust
let offline = Memo::new(move |_| conn.get() != crate::offline::Connectivity::Online);
```

- `AddLinkForm`: render only when `!offline.get()`; otherwise `<p class="muted">"Offline — showing saved links, editing disabled."</p>`.
- Link rows: edit/delete/archive buttons hidden when offline (pass `offline` down or gate where the buttons render; find them by grepping for `archive_link` / `editing_link.set` in links.rs).
- Collection sidebar: add/edit buttons hidden when offline (`CollectionSidebar` gets an `offline: Memo<bool>` or `Signal<bool>` prop).
- The search input stays enabled; a search while offline hits the `unavailable offline` arm and renders the existing error slot — make that message friendly: in the `Some(Err(err))` arm, when offline show `"Not available offline"` instead of the raw transport text.

- [ ] **Step 3: Unit-test the default-query predicate**

In links.rs tests module:

```rust
#[test]
fn only_the_pristine_first_page_counts_as_default() {
    let default = effective_query(None, None, None, None, 0);
    assert!(is_default_query(&default));
    let searched = effective_query(None, None, None, Some("rust".into()), 0);
    assert!(!is_default_query(&searched));
    let paged = effective_query(None, None, None, None, 2);
    assert!(!is_default_query(&paged));
}
```

(Adapt to `effective_query`'s real signature — read it first.)

- [ ] **Step 4: Test, commit**

Run: `cargo test --workspace && cargo build -p chaos-ui --target wasm32-unknown-unknown && cargo clippy --workspace --all-targets -- -D warnings`

```bash
git add crates/chaos-ui && git -c commit.gpgsign=false commit -m "feat(ui): links page browsable read-only offline"
```

---

### Task 7: Calendar, Home and quick-search offline states

**Files:**
- Modify: `crates/chaos-ui/src/pages/calendar.rs`
- Modify: `crates/chaos-ui/src/pages/home.rs`
- Modify: `crates/chaos-ui/src/search.rs`

- [ ] **Step 1: Calendar cached reads, mutations disabled**

In `CalendarPage` (`calendar.rs:110-126`): wrap `calendar_events` in `cached(conn, &format!("calendar:{year}-{month:02}"), …)` (derive year/month from the month signal the resource already keys on) and `list_calendars` in `cached(conn, "calendars", …)`, dropping stale flags. Track `conn` in both closures. Gate the "new event" / edit affordances and the refresh button on `conn == Online` with a muted "Offline — read-only" hint near the header. Careful with the existing 401 handling: `cached` passes `ClientError::Api` through untouched, so the sign-in prompt path is unchanged.

- [ ] **Step 2: Home cached reads, light toggles disabled**

In `home.rs:24-45`: wrap `home_temperature` with key `format!("home:temp:{range:?}")` (the range enum/struct the resource keys on — use its Display/Debug), `home_lights` with `"home:lights"`, `home_sensors` with `"home:sensors"`. Track `conn`. When offline, `LightCard` renders state but the toggle is disabled (`disabled=move || offline.get()`); stale light states get the same "· cached" treatment as dashboard widgets if cheap, otherwise skip (lights offline are decorative).

- [ ] **Step 3: Quick-search offline message**

In `search.rs`, the results `LocalResource` (`search.rs:46-55`): before fetching, check connectivity:

```rust
let conn = crate::offline::use_connectivity();
let results = LocalResource::new(move || {
    let q = debounced.get();
    let client = client.clone();
    let online = conn.get() == crate::offline::Connectivity::Online;
    async move {
        if q.trim().is_empty() {
            return Ok(SearchResults::default());
        }
        if !online {
            return Err(chaos_client::ClientError::Transport(
                "search is unavailable offline".into(),
            ));
        }
        client.search(&q).await
    }
});
```

In the `Some(Err(err))` render arm, show the message plainly (`"Search is unavailable offline"`) when it is that error — matching on the message string is fine here (kept local to this file).

- [ ] **Step 4: Test, commit**

Run: `cargo test --workspace && cargo build -p chaos-ui --target wasm32-unknown-unknown && cargo clippy --workspace --all-targets -- -D warnings`

```bash
git add crates/chaos-ui && git -c commit.gpgsign=false commit -m "feat(ui): calendar/home/search offline states"
```

---

### Task 8: Docs

**Files:**
- Modify: `docs/ROADMAP.md` (add/tick an offline-support entry under the current phase)
- Modify: `docs/deployment.md` only if it documents client behavior (probably not — check)

- [ ] **Step 1: Update docs, commit**

Describe: connectivity model (probe-decided), what is cached, read-only offline semantics, badge recovery.

```bash
git add docs && git -c commit.gpgsign=false commit -m "docs: offline core behavior"
```
