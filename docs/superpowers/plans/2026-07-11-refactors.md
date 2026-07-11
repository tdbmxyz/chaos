# Review Refactors Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Execute the behavior-preserving refactors identified in review: a shared bounded `StaleCache` on the server, shared month-grid/date helpers and polling/action hooks in the UI, and a batch of small dedupes across both crates.

**Architecture:** Rust workspace at `/projects/rust/chaos`. `crates/chaos-server` is an Axum server (tokio, sqlx/SQLite, reqwest); `crates/chaos-ui` is a Leptos 0.8 CSR crate shared by the trunk web bundle and the Tauri shell. Wire types live in `crates/chaos-domain`. Every task extracts an existing hand-rolled pattern into one helper, covers the helper with unit tests, then migrates call sites under the existing test suites.

**Tech Stack:** Rust (edition 2024 features like let-chains are in use), Axum, tokio, sqlx, reqwest, Leptos 0.8 (CSR), chrono, uuid.

---

## Preconditions and executor notes — READ FIRST

- **This plan runs AFTER the robustness plan.** That plan created `crates/chaos-server/src/http_util.rs` with `get_json` / `get_body_capped` helpers and migrated body fetching in `widgets/feed.rs`, `widgets/releases.rs`, `ics.rs`, `widgets/weather.rs`, `widgets/posts.rs` to it. Consequently, exact line numbers in those files have drifted from what this plan's author read. **Where this plan cites line numbers, treat them as approximate; locate code by function and structure and adapt.** Do NOT re-introduce inline `resp.bytes()` / `resp.json()` fetching — leave the `http_util` calls exactly as the robustness plan left them; this plan only touches cache mechanics, not fetching.
- **A prior UX-bugs plan stabilized the `Collapsible` state in `crates/chaos-ui/src/pages/dashboard.rs` across polls.** If `Collapsible` or its call sites look different from the snippets below (e.g. state keyed per widget, `StoredValue`, extra props), preserve that fix verbatim — Task 3 only replaces the interval/version/busy/error scaffolding around the resources and callbacks, never the `Collapsible` usage.
- All commands run from `/projects/rust/chaos`.
- Every commit message ends with these two trailer lines (exactly, each on its own line):
  ```
  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
  ```
- Run tests green **before** each change (baseline) and green **after**. If the baseline is already red, stop and report — do not build on a broken tree.

---

### Task 1: Generic `StaleCache<K, V>` on the server

The TTL-cache + serve-stale-on-failure pattern is hand-rolled three times: `WidgetHub::data` and `WidgetHub::weather` in `crates/chaos-server/src/widgets/mod.rs` (near line-for-line duplicates over `cache: RwLock<HashMap<String, CacheEntry>>`), and `FeedCache::raw_events` in `crates/chaos-server/src/ics.rs` (over `RwLock<HashMap<Uuid, (Instant, Arc<Vec<RawEvent>>)>>`). The widget cache and the geocode cache are also **unbounded and keyed by user-controlled `?location=` strings** (`data()`'s `format!("{id}@{place}")` keys, `weather()`'s `format!("weather@{place}")` keys, and `weather::resolve`'s geocode insertions). A bounded shared cache fixes both.

**Files:**
- Create: `crates/chaos-server/src/cache.rs`
- Modify: `crates/chaos-server/src/main.rs` (add `mod cache;`)
- Modify: `crates/chaos-server/src/widgets/mod.rs` (`WidgetHub` fields, `data`, `weather`, `systemd_action`, new `cached_fetch`)
- Modify: `crates/chaos-server/src/widgets/weather.rs` (`resolve` / `fetch` signature: geocode cache type)
- Modify: `crates/chaos-server/src/ics.rs` (`FeedCache` fields, `raw_events`, `invalidate`)
- Test: unit tests inside `crates/chaos-server/src/cache.rs`; existing suites in `widgets/mod.rs`, `ics.rs`

- [ ] **Step 1.1: Baseline — run the server suite green**

Run: `cargo test -p chaos-server`
Expected: PASS (all existing tests, including `widgets::tests::*` and `ics::tests::*`).

- [ ] **Step 1.2: Write `crates/chaos-server/src/cache.rs` with its unit tests**

Create the file with this complete content:

```rust
//! A small bounded TTL cache that can also serve stale entries when a
//! refresh fails. Shared by the widget hub (widget payloads, geocoding)
//! and the ICS feed cache — the hand-rolled versions of this pattern were
//! unbounded, which mattered because weather cache keys come from the
//! user-controlled `?location=` query.

use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

pub struct StaleCache<K, V> {
    max_entries: usize,
    inner: RwLock<Inner<K, V>>,
}

struct Inner<K, V> {
    entries: HashMap<K, Entry<V>>,
    /// Monotonic insertion counter: the entry with the smallest `seq` is
    /// the oldest-written and gets evicted first. (An `Instant` could tie;
    /// a counter cannot.)
    seq: u64,
}

struct Entry<V> {
    value: V,
    inserted: Instant,
    seq: u64,
}

impl<K: Eq + Hash + Clone, V: Clone> StaleCache<K, V> {
    pub fn new(max_entries: usize) -> Self {
        assert!(max_entries > 0, "cache must hold at least one entry");
        Self {
            max_entries,
            inner: RwLock::new(Inner {
                entries: HashMap::new(),
                seq: 0,
            }),
        }
    }

    /// The cached value if it is younger than `ttl`.
    pub async fn get_fresh(&self, key: &K, ttl: Duration) -> Option<V> {
        let inner = self.inner.read().await;
        let entry = inner.entries.get(key)?;
        (entry.inserted.elapsed() < ttl).then(|| entry.value.clone())
    }

    /// The cached value regardless of age (serve-stale-on-failure).
    pub async fn get_stale(&self, key: &K) -> Option<V> {
        self.inner
            .read()
            .await
            .entries
            .get(key)
            .map(|entry| entry.value.clone())
    }

    /// Insert or refresh a value. When the cache is full and the key is
    /// new, the oldest-written entry is evicted, bounding growth from
    /// user-controlled keys.
    pub async fn insert(&self, key: K, value: V) {
        let mut inner = self.inner.write().await;
        inner.seq += 1;
        let seq = inner.seq;
        if !inner.entries.contains_key(&key) && inner.entries.len() >= self.max_entries {
            if let Some(oldest) = inner
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.seq)
                .map(|(key, _)| key.clone())
            {
                inner.entries.remove(&oldest);
            }
        }
        inner.entries.insert(
            key,
            Entry {
                value,
                inserted: Instant::now(),
                seq,
            },
        );
    }

    /// Forget one entry (cache invalidation after an edit/delete).
    pub async fn remove(&self, key: &K) {
        self.inner.write().await.entries.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fresh_within_ttl_stale_after() {
        let cache = StaleCache::new(4);
        cache.insert("k", 1u32).await;
        assert_eq!(cache.get_fresh(&"k", Duration::from_secs(60)).await, Some(1));
        // A zero TTL makes any entry stale without sleeping.
        assert_eq!(cache.get_fresh(&"k", Duration::ZERO).await, None);
        assert_eq!(cache.get_stale(&"k").await, Some(1));
    }

    #[tokio::test]
    async fn missing_keys_yield_nothing() {
        let cache: StaleCache<&str, u32> = StaleCache::new(4);
        assert_eq!(cache.get_fresh(&"nope", Duration::from_secs(60)).await, None);
        assert_eq!(cache.get_stale(&"nope").await, None);
    }

    #[tokio::test]
    async fn eviction_drops_the_oldest_entry() {
        let cache = StaleCache::new(2);
        cache.insert("a", 1u32).await;
        cache.insert("b", 2).await;
        cache.insert("c", 3).await; // evicts "a"
        assert_eq!(cache.get_stale(&"a").await, None);
        assert_eq!(cache.get_stale(&"b").await, Some(2));
        assert_eq!(cache.get_stale(&"c").await, Some(3));
    }

    #[tokio::test]
    async fn refreshing_a_key_does_not_evict() {
        let cache = StaleCache::new(2);
        cache.insert("a", 1u32).await;
        cache.insert("b", 2).await;
        cache.insert("a", 10).await; // refresh in place, still 2 entries
        assert_eq!(cache.get_stale(&"a").await, Some(10));
        assert_eq!(cache.get_stale(&"b").await, Some(2));
    }

    #[tokio::test]
    async fn remove_forgets_the_entry() {
        let cache = StaleCache::new(2);
        cache.insert("a", 1u32).await;
        cache.remove(&"a").await;
        assert_eq!(cache.get_stale(&"a").await, None);
    }
}
```

Then register the module in `crates/chaos-server/src/main.rs` — the module list at the top currently reads `mod api; mod archiver; mod auth; mod config; …` (plus `mod http_util;` from the robustness plan); insert `mod cache;` alphabetically after `mod auth;`.

- [ ] **Step 1.3: Run the new cache tests**

Run: `cargo test -p chaos-server cache::`
Expected: PASS — 5 tests (`fresh_within_ttl_stale_after`, `missing_keys_yield_nothing`, `eviction_drops_the_oldest_entry`, `refreshing_a_key_does_not_evict`, `remove_forgets_the_entry`). If the compiler flags `mod cache` as dead code (nothing uses it yet), that is only a warning; migration follows in the next steps.

- [ ] **Step 1.4: Commit the cache module**

```bash
git add crates/chaos-server/src/cache.rs crates/chaos-server/src/main.rs
git commit -m "$(cat <<'EOF'
refactor(server): add bounded StaleCache with serve-stale semantics

Extracts the TTL + stale-fallback cache pattern hand-rolled in the
widget hub and the ICS feed cache; bounded with oldest-first eviction
so user-controlled keys cannot grow it without limit.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

- [ ] **Step 1.5: Migrate `WidgetHub` (widgets/mod.rs) and the geocode cache (widgets/weather.rs)**

In `crates/chaos-server/src/widgets/mod.rs`:

1. Replace the imports `use std::time::{Duration, Instant};` → `use std::time::Duration;`, drop `use tokio::sync::RwLock;` (no longer used here), and add `use crate::cache::StaleCache;`. Keep `use std::collections::HashMap;` (still used by `entries` and `resolve_layout`).
2. Delete the `struct CacheEntry { fetched: Instant, data: WidgetData }` definition.
3. Change the `WidgetHub` fields:

```rust
    cache: StaleCache<String, WidgetData>,
    /// Geocoded weather locations. Bounded because keys come from the
    /// user-controlled `?location=` query; entries never expire (a city's
    /// coordinates don't move), they only rotate out on overflow.
    geocode: StaleCache<String, weather::Place>,
```

4. In `WidgetHub::new`, replace the two `RwLock::new(HashMap::new())` initializers:

```rust
            cache: StaleCache::new(WIDGET_CACHE_ENTRIES),
            geocode: StaleCache::new(GEOCODE_CACHE_ENTRIES),
```

and add these constants next to the existing `ttl` fn (module level):

```rust
/// Caps on cache growth: both caches take user-controlled `?location=`
/// keys, so they must be bounded. Generous for any real dashboard.
const WIDGET_CACHE_ENTRIES: usize = 512;
const GEOCODE_CACHE_ENTRIES: usize = 256;
```

5. Add the shared fetch-through-cache helper inside `impl WidgetHub` (this is the collapsed body of the old `data`/`weather` duplication):

```rust
    /// Fetch-through-cache with the hub's staleness rules: serve a cached
    /// payload within `ttl`; on upstream failure prefer a stale payload
    /// over an error.
    async fn cached_fetch<F>(
        &self,
        cache_key: String,
        ttl: Duration,
        fetch: F,
    ) -> Result<WidgetData, WidgetError>
    where
        F: Future<Output = Result<WidgetData, String>>,
    {
        if let Some(data) = self.cache.get_fresh(&cache_key, ttl).await {
            return Ok(data);
        }
        match fetch.await {
            Ok(data) => {
                self.cache.insert(cache_key, data.clone()).await;
                Ok(data)
            }
            Err(reason) => {
                if let Some(data) = self.cache.get_stale(&cache_key).await {
                    tracing::warn!(key = cache_key, reason, "refresh failed, serving stale data");
                    return Ok(data);
                }
                tracing::warn!(key = cache_key, reason, "fetch failed");
                Err(WidgetError::Upstream(reason))
            }
        }
    }
```

(If the prelude's `Future` is not in scope under the crate's edition, add `use std::future::Future;` to the imports.) Note the log field changes from `id`/`place` to `key` — the key embeds the id/place, so no information is lost.

6. Rewrite `data` — keep the doc comment, replace the body from the cache lookup down:

```rust
    pub async fn data(&self, id: &str, location: Option<&str>) -> Result<WidgetData, WidgetError> {
        let widget = self.entries.get(id).ok_or(WidgetError::UnknownWidget)?;
        let ttl = ttl(widget);

        let location = location
            .map(str::trim)
            .filter(|l| !l.is_empty() && l.len() <= 64 && matches!(widget, Widget::Weather { .. }));
        let cache_key = match location {
            Some(place) => format!("{id}@{place}"),
            None => id.to_string(),
        };

        self.cached_fetch(cache_key, ttl, self.fetch(widget, location))
            .await
    }
```

7. Rewrite `weather` — keep the doc comment and the location/configured resolution, replace the cache/fetch tail:

```rust
    pub async fn weather(
        &self,
        location: Option<&str>,
    ) -> Result<chaos_domain::WeatherData, WidgetError> {
        let location = location
            .map(str::trim)
            .filter(|l| !l.is_empty() && l.len() <= 64);
        let configured = self.entries.values().find_map(|w| match w {
            Widget::Weather { location } => Some(location.as_str()),
            _ => None,
        });
        let place = location.or(configured).ok_or_else(|| {
            WidgetError::Rejected("no location given and no weather widget configured".into())
        })?;

        let data = self
            .cached_fetch(
                format!("weather@{place}"),
                Duration::from_secs(600),
                weather::fetch(&self.http, &self.geocode, place),
            )
            .await?;
        match data {
            WidgetData::Weather(data) => Ok(data),
            _ => unreachable!("weather cache keys only hold weather data"),
        }
    }
```

(The old code matched `WidgetData::Weather` on the cache entry inline; `weather@…` keys are only ever written by this function with weather payloads, so unwrapping after the shared helper preserves behavior — the old non-weather arm was equally `unreachable!`.)

8. In `systemd_action`, replace the trailing cache write

```rust
        self.cache.write().await.insert(
            id.to_string(),
            CacheEntry { fetched: Instant::now(), data: data.clone() },
        );
```

with:

```rust
        self.cache.insert(id.to_string(), data.clone()).await;
```

In `crates/chaos-server/src/widgets/weather.rs`:

1. Drop `use std::collections::HashMap;` and `use tokio::sync::RwLock;` (if nothing else uses them after this change) and add `use crate::cache::StaleCache;`.
2. Change the signatures of `fetch` and `resolve` from `geocode_cache: &RwLock<HashMap<String, Place>>` / `cache: &RwLock<HashMap<String, Place>>` to:

```rust
pub async fn fetch(
    http: &reqwest::Client,
    geocode_cache: &StaleCache<String, Place>,
    location: &str,
) -> Result<WidgetData, String> {
```

```rust
async fn resolve(
    http: &reqwest::Client,
    cache: &StaleCache<String, Place>,
    location: &str,
) -> Result<Place, String> {
```

3. In `resolve`, replace the read `if let Some(place) = cache.read().await.get(location) { return Ok(place.clone()); }` with:

```rust
    if let Some(place) = cache.get_stale(location).await {
        return Ok(place);
    }
```

(`get_stale`, deliberately: geocode entries were cached "for the process lifetime" before, so age never matters — only the bound does.)

4. Replace the write `cache.write().await.insert(location.to_string(), place.clone());` with:

```rust
    cache.insert(location.to_string(), place.clone()).await;
```

- [ ] **Step 1.6: Run the widget tests green**

Run: `cargo test -p chaos-server widgets::`
Expected: PASS — all existing `widgets::mod` tests (`default_layout_mirrors_legacy_config`, `configured_columns_get_stable_ids_and_fallbacks`, `systemd_action_enforces_the_allowlist`, `columns_parse_from_toml_config`) and `widgets::weather` tests (`now_index_*`, `forecast_tolerates_null_tail_entries`) unchanged and green.

- [ ] **Step 1.7: Migrate `FeedCache` (ics.rs)**

In `crates/chaos-server/src/ics.rs` (remember: `fetch` may already delegate to `crate::http_util::get_body_capped` — do not touch it):

1. Drop `use std::collections::HashMap;`, drop `use tokio::sync::RwLock;`, change `use std::time::{Duration, Instant};` → `use std::time::Duration;`, and add `use crate::cache::StaleCache;`.
2. Delete the `type CachedFeed = (Instant, Arc<Vec<RawEvent>>);` alias. Add near the `TTL` const:

```rust
/// Cap on cached feeds; keys are the user's calendar ids, so this is
/// effectively "how many feed calendars one server realistically has".
const MAX_FEEDS: usize = 128;
```

3. Change the struct and `Default`:

```rust
pub struct FeedCache {
    entries: StaleCache<Uuid, Arc<Vec<RawEvent>>>,
    http: reqwest::Client,
}
```

with `entries: StaleCache::new(MAX_FEEDS),` replacing `entries: RwLock::new(HashMap::new()),` in `Default::default` (keep the `http` builder exactly as-is).

4. Rewrite `raw_events`:

```rust
    async fn raw_events(&self, calendar_id: Uuid, url: &str) -> Result<Arc<Vec<RawEvent>>, String> {
        if let Some(events) = self.entries.get_fresh(&calendar_id, TTL).await {
            return Ok(events);
        }

        match self.fetch(url).await {
            Ok(events) => {
                let events = Arc::new(events);
                self.entries.insert(calendar_id, events.clone()).await;
                Ok(events)
            }
            Err(reason) => {
                // Serve the stale copy if we have one.
                if let Some(events) = self.entries.get_stale(&calendar_id).await {
                    tracing::warn!(%calendar_id, reason, "ics refresh failed, serving stale feed");
                    return Ok(events);
                }
                Err(reason)
            }
        }
    }
```

5. Rewrite `invalidate`:

```rust
    /// Drop a cached feed (after its calendar is edited or deleted).
    pub async fn invalidate(&self, calendar_id: Uuid) {
        self.entries.remove(&calendar_id).await;
    }
```

- [ ] **Step 1.8: Run the full server suite green**

Run: `cargo test -p chaos-server`
Expected: PASS — everything, including `ics::tests::parses_and_expands_a_feed` and `ics::tests::range_filters_out_of_window_occurrences`. Also run `cargo clippy -p chaos-server` if clippy is part of the repo's habits; at minimum there must be no warnings about unused imports you should have removed.

- [ ] **Step 1.9: Commit the migration**

```bash
git add crates/chaos-server/src/widgets/mod.rs crates/chaos-server/src/widgets/weather.rs crates/chaos-server/src/ics.rs
git commit -m "$(cat <<'EOF'
refactor(server): migrate widget hub and feed cache onto StaleCache

Collapses the duplicated TTL + serve-stale logic in WidgetHub::data /
WidgetHub::weather into one cached_fetch helper and moves FeedCache to
the same primitive. The widget and geocode caches are now bounded, so
user-controlled ?location= keys can no longer grow them unboundedly.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

---

### Task 2: Shared month-grid/date helpers in the UI

The "6 fixed Monday-first weeks" computation exists three times (`calendar_cells` in `crates/chaos-ui/src/pages/dashboard.rs`, `grid_utc_range` and `MonthGrid` in `crates/chaos-ui/src/pages/calendar.rs`) and the month-shift closure is duplicated verbatim in `CalendarWidget` (dashboard.rs) and `CalendarView` (calendar.rs).

**Files:**
- Create: `crates/chaos-ui/src/date_util.rs`
- Modify: `crates/chaos-ui/src/lib.rs` (add `mod date_util;`)
- Modify: `crates/chaos-ui/src/pages/dashboard.rs` (`CalendarWidget`'s `shift`, `calendar_cells`)
- Modify: `crates/chaos-ui/src/pages/calendar.rs` (`CalendarView`'s `shift`, `grid_utc_range`, `MonthGrid`)
- Test: unit tests inside `crates/chaos-ui/src/date_util.rs` (native — the crate already runs native tests in `pages/home.rs`, `pages/weather.rs`, `echarts.rs`, `components.rs`)

- [ ] **Step 2.1: Baseline — run the UI suite green**

Run: `cargo test -p chaos-ui`
Expected: PASS (the existing native tests). Also run `cargo check -p chaos-ui` — clean.

- [ ] **Step 2.2: Write `crates/chaos-ui/src/date_util.rs` with its unit tests**

Create the file with this complete content:

```rust
//! Shared month-grid math for the dashboard calendar widget and the
//! calendar page: both render 6 fixed Monday-first weeks (42 days), and
//! both shift the shown (year, month) with the same arithmetic.

use chrono::{Datelike, Days, NaiveDate};

/// Days in the fixed 6-week grid.
pub(crate) const GRID_DAYS: u64 = 42;

/// The Monday starting the 6-week grid that shows (year, month).
/// `None` only for dates outside chrono's supported range — unreachable
/// through the UI, where months always come from [`shift_month`].
pub(crate) fn grid_start(year: i32, month: u32) -> Option<NaiveDate> {
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    Some(first - Days::new(first.weekday().num_days_from_monday() as u64))
}

/// The 42 days of the grid, Monday-first, oldest first.
pub(crate) fn month_grid(year: i32, month: u32) -> Option<impl Iterator<Item = NaiveDate>> {
    let start = grid_start(year, month)?;
    Some((0..GRID_DAYS).map(move |i| start + Days::new(i)))
}

/// (year, month) moved by `delta` months, carrying across year boundaries.
pub(crate) fn shift_month(year: i32, month: u32, delta: i32) -> (i32, u32) {
    let total = year * 12 + (month as i32 - 1) + delta;
    (total.div_euclid(12), (total.rem_euclid(12) + 1) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Weekday;

    #[test]
    fn grid_starts_on_the_monday_before_the_first() {
        // July 2026 starts on a Wednesday; the grid opens Mon June 29.
        let start = grid_start(2026, 7).unwrap();
        assert_eq!(start, NaiveDate::from_ymd_opt(2026, 6, 29).unwrap());
        assert_eq!(start.weekday(), Weekday::Mon);
        // June 2026 starts ON a Monday: no back-fill.
        assert_eq!(
            grid_start(2026, 6).unwrap(),
            NaiveDate::from_ymd_opt(2026, 6, 1).unwrap()
        );
        // Out-of-range months are None, not a panic.
        assert!(grid_start(2026, 13).is_none());
    }

    #[test]
    fn grid_covers_42_days_across_a_year_boundary() {
        // January 2026 starts on a Thursday: the grid runs Mon Dec 29 2025
        // through Sun Feb 8 2026.
        let days: Vec<NaiveDate> = month_grid(2026, 1).unwrap().collect();
        assert_eq!(days.len(), 42);
        assert_eq!(days[0], NaiveDate::from_ymd_opt(2025, 12, 29).unwrap());
        assert_eq!(days[41], NaiveDate::from_ymd_opt(2026, 2, 8).unwrap());
    }

    #[test]
    fn february_grid_handles_short_months() {
        // February 2026 starts on a Sunday: grid opens Mon Jan 26 and the
        // short month still fills all 42 cells, ending Sun Mar 8.
        let days: Vec<NaiveDate> = month_grid(2026, 2).unwrap().collect();
        assert_eq!(days[0], NaiveDate::from_ymd_opt(2026, 1, 26).unwrap());
        assert_eq!(days[41], NaiveDate::from_ymd_opt(2026, 3, 8).unwrap());
    }

    #[test]
    fn shift_month_carries_across_years() {
        assert_eq!(shift_month(2026, 1, -1), (2025, 12));
        assert_eq!(shift_month(2025, 12, 1), (2026, 1));
        assert_eq!(shift_month(2026, 7, 0), (2026, 7));
        assert_eq!(shift_month(2026, 7, -19), (2024, 12));
        assert_eq!(shift_month(2026, 7, 18), (2028, 1));
    }
}
```

Register it in `crates/chaos-ui/src/lib.rs`: the module list currently reads `mod components; mod echarts; mod pages;` — insert `mod date_util;` after `mod components;`.

- [ ] **Step 2.3: Run the new date tests**

Run: `cargo test -p chaos-ui date_util`
Expected: PASS — 4 tests. (Dead-code warnings until migration are fine.)

- [ ] **Step 2.4: Commit the helper module**

```bash
git add crates/chaos-ui/src/date_util.rs crates/chaos-ui/src/lib.rs
git commit -m "$(cat <<'EOF'
refactor(ui): extract month-grid and month-shift date helpers

The 6-week Monday-first grid math existed three times (dashboard
calendar widget, calendar page range + grid) and the month-shift
closure twice; date_util now owns both, with native unit tests.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

- [ ] **Step 2.5: Migrate the dashboard call sites (pages/dashboard.rs)**

1. In `CalendarWidget`, replace the `shift` closure body

```rust
    let shift = move |delta: i32| {
        month.update(|(year, m)| {
            let total = *year * 12 + (*m as i32 - 1) + delta;
            *year = total.div_euclid(12);
            *m = (total.rem_euclid(12) + 1) as u32;
        });
    };
```

with:

```rust
    let shift = move |delta: i32| {
        month.update(|(year, m)| {
            (*year, *m) = crate::date_util::shift_month(*year, *m, delta);
        });
    };
```

2. Replace `calendar_cells` in full:

```rust
/// Six fixed weeks around the shown month, starting on Monday.
fn calendar_cells((year, month): (i32, u32), today: NaiveDate) -> impl IntoView {
    let Some(days) = crate::date_util::month_grid(year, month) else {
        return ().into_any();
    };
    days.map(|date| {
        let mut class = String::from("calendar-cell");
        if date.month() != month {
            class.push_str(" other");
        }
        if date == today {
            class.push_str(" today");
        }
        view! { <span class=class>{date.day()}</span> }
    })
    .collect_view()
    .into_any()
}
```

3. Remove now-unused imports from the top of dashboard.rs as the compiler indicates (`Days` was only used by the old `calendar_cells`; `Datelike` is still needed by `CalendarWidget`'s `today.year()`/`today.month()` calls — let `cargo check` be the judge).

- [ ] **Step 2.6: Migrate the calendar-page call sites (pages/calendar.rs)**

1. In `CalendarView`, replace the identical `shift` closure with the same four-line version as Step 2.5 item 1.
2. Replace `grid_utc_range` in full:

```rust
/// UTC range covering the 6-week grid shown for (year, month), in local time.
fn grid_utc_range((year, month): (i32, u32)) -> (DateTime<Utc>, DateTime<Utc>) {
    // The fallback is unreachable through the UI (shift_month only produces
    // valid months); NaiveDate::default() keeps the function total.
    let start = crate::date_util::grid_start(year, month).unwrap_or_default();
    (
        local_midnight(start),
        local_midnight(start + Days::new(crate::date_util::GRID_DAYS)),
    )
}
```

(Behavior note: the old unreachable fallback anchored on `NaiveDate::default()`'s Monday-shifted date; the new one anchors on `NaiveDate::default()` itself. Both are impossible to hit — `shift_month` always yields a valid month — so this is acceptable.)

3. In `MonthGrid`, replace

```rust
    let first = NaiveDate::from_ymd_opt(month.0, month.1, 1).unwrap_or_default();
    let start = first - Days::new(first.weekday().num_days_from_monday() as u64);
```

with:

```rust
    let start = crate::date_util::grid_start(month.0, month.1).unwrap_or_default();
```

and change both `for i in 0..42u64` / `(0..42u64)` loops in `MonthGrid` to `for i in 0..crate::date_util::GRID_DAYS` / `(0..crate::date_util::GRID_DAYS)`. Everything else in `MonthGrid` (the `by_day` map, `covers`, classes, chips) stays byte-identical.

4. Remove imports the compiler now flags as unused (likely `Weekday`-free already; `Datelike` may still be needed by `day.month()`/`day.day()` — again, let `cargo check` decide).

- [ ] **Step 2.7: Run the UI suite green**

Run: `cargo test -p chaos-ui && cargo check -p chaos-ui`
Expected: PASS, no warnings about unused imports.

- [ ] **Step 2.8: Commit the migration**

```bash
git add crates/chaos-ui/src/pages/dashboard.rs crates/chaos-ui/src/pages/calendar.rs
git commit -m "$(cat <<'EOF'
refactor(ui): use date_util grid/shift helpers in both calendars

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

---

### Task 3: UI polling/action scaffolding (`hooks.rs`)

`ServicesWidget`, `SystemdWidget` and `DataWidget` in `crates/chaos-ui/src/pages/dashboard.rs` each hand-roll: an interval tick signal + `on_cleanup`, a version-bump signal, busy/error signals, and a `Callback` wrapping `spawn_local`. Extract `use_interval_tick`, `use_polled_resource` and `use_action` into `crates/chaos-ui/src/hooks.rs`, moving `RefreshTick` there so the hook can track it.

**REMINDER:** the UX-bugs plan stabilized `Collapsible` state across polls. Whatever `Collapsible` looks like now, keep it — only the scaffolding around resources/callbacks changes.

**Files:**
- Create: `crates/chaos-ui/src/hooks.rs`
- Modify: `crates/chaos-ui/src/lib.rs` (add `mod hooks;`)
- Modify: `crates/chaos-ui/src/pages/dashboard.rs` (`Dashboard`, `ServicesWidget`, `DataWidget`, `SystemdWidget`; delete the local `RefreshTick`)
- Test: `cargo check -p chaos-ui` + existing `cargo test -p chaos-ui` suites (the hooks are reactive-runtime glue; no pure logic worth a native harness — the pure pieces, date math and formatting, are tested in Tasks 2 and 4)

- [ ] **Step 3.1: Baseline**

Run: `cargo test -p chaos-ui && cargo check -p chaos-ui`
Expected: PASS.

- [ ] **Step 3.2: Write `crates/chaos-ui/src/hooks.rs`**

Create the file with this complete content:

```rust
//! Reusable reactive scaffolding for the dashboard widgets: interval-driven
//! polling and busy/error bookkeeping around fire-and-forget actions.

use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;

/// Bumped by the dashboard's manual refresh button; every polled resource
/// tracks it when it is in context.
#[derive(Clone, Copy)]
pub(crate) struct RefreshTick(pub(crate) RwSignal<u32>);

/// A counter signal bumped every `interval` for as long as the current
/// reactive owner lives.
pub(crate) fn use_interval_tick(interval: Duration) -> RwSignal<u32> {
    let tick = RwSignal::new(0u32);
    if let Ok(handle) = set_interval_with_handle(move || tick.update(|n| *n += 1), interval) {
        on_cleanup(move || handle.clear());
    }
    tick
}

/// A [`LocalResource`] re-run every `interval`, whenever the dashboard-wide
/// [`RefreshTick`] bumps, and whenever `version` (an action's success
/// counter, see [`use_action`]) changes. Pass `None` for resources without
/// a mutating action.
pub(crate) fn use_polled_resource<T, Fut>(
    interval: Duration,
    version: Option<RwSignal<u32>>,
    fetch: impl Fn() -> Fut + Send + Sync + 'static,
) -> LocalResource<T>
where
    T: 'static,
    Fut: Future<Output = T> + 'static,
{
    let tick = use_interval_tick(interval);
    let refresh = use_context::<RefreshTick>();
    LocalResource::new(move || {
        tick.track();
        if let Some(version) = version {
            version.track();
        }
        if let Some(RefreshTick(refresh)) = refresh {
            refresh.track();
        }
        fetch()
    })
}

/// Signals around an async action: `busy` while it runs, `error` carrying
/// the last failure, `version` bumped on success so polled resources
/// refetch right away instead of on the next poll.
#[derive(Clone, Copy)]
pub(crate) struct ActionState {
    pub version: RwSignal<u32>,
    pub busy: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
}

/// Wrap an async operation in busy/error bookkeeping; returns the state
/// plus the [`Callback`] to hand to buttons.
pub(crate) fn use_action<I, Fut, T, E>(
    run: impl Fn(I) -> Fut + Send + Sync + 'static,
) -> (ActionState, Callback<I>)
where
    I: Send + 'static,
    Fut: Future<Output = Result<T, E>> + 'static,
    T: 'static,
    E: std::fmt::Display + 'static,
{
    let state = ActionState {
        version: RwSignal::new(0u32),
        busy: RwSignal::new(false),
        error: RwSignal::new(None),
    };
    let callback = Callback::new(move |input: I| {
        let fut = run(input);
        state.busy.set(true);
        state.error.set(None);
        spawn_local(async move {
            match fut.await {
                Ok(_) => state.version.update(|n| *n += 1),
                Err(err) => state.error.set(Some(err.to_string())),
            }
            state.busy.set(false);
        });
    });
    (state, callback)
}
```

Register in `crates/chaos-ui/src/lib.rs`: add `mod hooks;` after `mod echarts;`.

**Adapt if needed:** the `Send + Sync` bounds on `fetch`/`run`/`I` mirror what `LocalResource::new` / `Callback::new` demand in this Leptos 0.8 build (the existing widgets compile with `ChaosClient` captures, so the client satisfies them). If the compiler reports different bounds, follow the compiler — the shape (create the future *before* `spawn_local`, keep the future itself non-`Send`) must stay.

- [ ] **Step 3.3: Migrate the three widgets in pages/dashboard.rs**

1. Delete the local `RefreshTick` definition (`#[derive(Clone, Copy)] struct RefreshTick(RwSignal<u32>);`) and in `Dashboard` change `provide_context(RefreshTick(refresh));` to `provide_context(crate::hooks::RefreshTick(refresh));`.

2. Replace `ServicesWidget` in full (adapt the `Collapsible` block to whatever the UX-bugs plan left there):

```rust
/// The monitored-services grid, re-polled while the dashboard stays open.
#[component]
fn ServicesWidget() -> impl IntoView {
    let client = use_client();

    // Success bumps `action.version` so a started/stopped service's tile
    // flips to the fresh state right away instead of on the next poll.
    let (action, run) = crate::hooks::use_action({
        let client = client.clone();
        move |(id, verb): (String, SystemdAction)| {
            let client = client.clone();
            async move { client.service_action(&id, verb).await }
        }
    });

    let services = crate::hooks::use_polled_resource(SERVICES_REFRESH, Some(action.version), {
        let client = client.clone();
        move || {
            let client = client.clone();
            async move { client.services().await }
        }
    });

    view! {
        <section class="widget widget-services">
            <h2>"Services"</h2>
            {move || action.error.get().map(|err| view! { <p class="error">{err}</p> })}
            {move || match services.get() {
                None => view! { <p class="muted">"Checking services…"</p> }.into_any(),
                Some(Ok(list)) => {
                    let count = list.len();
                    view! {
                        <Collapsible count>
                            <ServiceGrid services=list controls=(action.busy, run)/>
                        </Collapsible>
                    }
                        .into_any()
                }
                Some(Err(err)) => {
                    view! { <p class="error">"Could not reach chaos server: " {err.to_string()}</p> }
                        .into_any()
                }
            }}
        </section>
    }
}
```

3. In `DataWidget`, delete the `tick` signal + `set_interval_with_handle` + `on_cleanup` block and the `refresh = use_context::<RefreshTick>()` line, and replace the `LocalResource::new(move || { tick.track(); … })` block with:

```rust
    let data = crate::hooks::use_polled_resource(WIDGET_REFRESH, None, {
        let client = client.clone();
        move || {
            let client = client.clone();
            let id = id.clone();
            let location = weather_location.clone().flatten();
            async move { client.widget_data(&id, location.as_deref()).await }
        }
    });
```

(`id` is the component prop; it was already moved into the old closure, so no other user exists — if the compiler disagrees, clone before the block. Everything else in `DataWidget` — kind/title computation, `weather_location`, the view match — stays untouched.)

4. In `SystemdWidget`, delete the tick/version/busy/action_error scaffolding and the `refresh` context read, replacing them with:

```rust
    // Unit states change on their own (crashes, timers), so poll like the
    // services grid does; a successful control action bumps the version.
    let (action, run) = crate::hooks::use_action({
        let client = client.clone();
        let id = id.clone();
        move |(unit, verb): (String, SystemdAction)| {
            let client = client.clone();
            let id = id.clone();
            async move {
                client
                    .systemd_action(&id, &SystemdActionRequest { unit, action: verb })
                    .await
            }
        }
    });

    let data = crate::hooks::use_polled_resource(SERVICES_REFRESH, Some(action.version), {
        let client = client.clone();
        let id = id.clone();
        move || {
            let client = client.clone();
            let id = id.clone();
            async move { client.widget_data(&id, None).await }
        }
    });
```

and in its view swap `action_error.get()` → `action.error.get()` and `Some((busy, run))` → `Some((action.busy, run))`. The rest of the view stays untouched.

5. Remove imports the compiler flags as unused in dashboard.rs (likely `leptos::task::spawn_local` if nothing else uses it — the search bar and bookmarks don't; check).

- [ ] **Step 3.4: Verify**

Run: `cargo check -p chaos-ui && cargo test -p chaos-ui`
Expected: clean check, all existing tests PASS. Manually re-read the diff of `Collapsible` and its call sites: it must be zero except for the `controls=(action.busy, run)` binding rename.

- [ ] **Step 3.5: Commit**

```bash
git add crates/chaos-ui/src/hooks.rs crates/chaos-ui/src/lib.rs crates/chaos-ui/src/pages/dashboard.rs
git commit -m "$(cat <<'EOF'
refactor(ui): extract polling/action hooks for dashboard widgets

use_polled_resource owns the interval + RefreshTick + action-version
tracking; use_action owns busy/error/version around spawn_local.
ServicesWidget, SystemdWidget and DataWidget migrate onto them; the
Collapsible poll-stability fix is untouched.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

---

### Task 4: Small dedupes

Seven independent cleanups, grouped into four commits: (a)+(b) server db, (c)+(d)+(e) UI, (f) API layout, (g) importer.

**Files:**
- Modify: `crates/chaos-server/src/db.rs`, `crates/chaos-server/src/db_calendar.rs` (imports only)
- Modify: `crates/chaos-ui/src/lib.rs`, `crates/chaos-ui/src/pages/dashboard.rs`, `crates/chaos-ui/src/pages/home.rs`, `crates/chaos-ui/src/pages/weather.rs`, `crates/chaos-ui/src/pages/more.rs`
- Create: `crates/chaos-server/src/api/services.rs`, `crates/chaos-server/src/api/widgets.rs`
- Modify: `crates/chaos-server/src/api/mod.rs`, `crates/chaos-server/src/api/calendar.rs`
- Modify: `crates/chaos-server/src/import.rs`

- [ ] **Step 4.1: Baseline**

Run: `cargo test -p chaos-server && cargo test -p chaos-ui`
Expected: PASS.

- [ ] **Step 4.2: (a) one `parse_uuid` + (b) shared `trimmed`**

(a) There are two `parse_uuid`s: `crates/chaos-server/src/db_auth.rs` has `pub(crate) fn parse_uuid(s: &str) -> Result<Uuid>` (the keeper — `db_calendar.rs` already imports it) and `crates/chaos-server/src/db.rs` has a private `fn parse_uuid(s: String) -> Result<Uuid>`. First run `grep -n "parse_uuid" crates/chaos-server/src/*.rs` to enumerate callers. Then:

1. Delete the `String`-taking `fn parse_uuid` from db.rs (next to `parse_url`, around line 602).
2. Add `use crate::db_auth::parse_uuid;` to db.rs's imports.
3. Fix db.rs call sites to pass `&str`:
   - `parse_uuid(row.get::<String, _>("id"))` (in `list_tags`) → `parse_uuid(&row.get::<String, _>("id"))`
   - direct field moves like `parse_uuid(row.id)` → `parse_uuid(&row.id)`
   - `row.parent_id.map(parse_uuid).transpose()` / `row.collection_id.map(parse_uuid).transpose()` / `row.created_by.map(...)` → `row.parent_id.as_deref().map(parse_uuid).transpose()` (same for the others)
   Let the compiler enumerate every site; the two fix shapes above cover them all.

(b) The trim-or-None pattern `X.as_deref().map(str::trim).filter(|s| !s.is_empty())` appears four times in db.rs (`create_collection` and `update_collection` on `req.description`; `create_link` and `update_link` on `req.description`) and the named helper already exists in `db_calendar.rs`:

```rust
fn trimmed(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|s| !s.is_empty())
}
```

1. Move it to db.rs (place it next to `validate_name`) as:

```rust
/// Trim-or-None: user-supplied optional text normalized for storage.
pub(crate) fn trimmed(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|s| !s.is_empty())
}
```

2. Delete the private copy from `db_calendar.rs` and add `trimmed` to its `use crate::db::{...}` import (it becomes `use crate::db::{Db, DbError, Result, trimmed};` — adjust to the actual current import line).
3. Replace all four inline occurrences in db.rs with `.bind(trimmed(&req.description))`.
4. Note: `create_link`'s *title* expression chains `.map(String::from).unwrap_or_else(...)` after the same prefix — you may rewrite it as `trimmed(&req.title).map(String::from).unwrap_or_else(|| ...)`, keeping the trailing logic identical.

Run: `cargo test -p chaos-server` — Expected: PASS (the db tests in db.rs/db_auth.rs/db_calendar.rs cover collection/link/calendar round-trips). Commit:

```bash
git add crates/chaos-server/src/db.rs crates/chaos-server/src/db_auth.rs crates/chaos-server/src/db_calendar.rs
git commit -m "$(cat <<'EOF'
refactor(server): single parse_uuid and shared trimmed() helper

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

- [ ] **Step 4.3: (c) temperature/details helpers — write helpers + tests first**

The °C→°F conversion is written out three times (`format_temp` in `crates/chaos-ui/src/lib.rs`, the `convert` closure in `chart_option` in `pages/home.rs`, `hourly_temps` in `pages/weather.rs`) and the "feels · wind · humidity" details string twice (`WeatherView` in `pages/dashboard.rs` with `weather.location` as lead, `weather_row_body` in `pages/weather.rs` with `weather.description` as lead).

In `crates/chaos-ui/src/lib.rs`, replace `format_temp` with this block (same location, after `set_weather_places`):

```rust
/// Celsius in the display unit: °F when the preference says so.
pub(crate) fn convert_temp(celsius: f64, fahrenheit: bool) -> f64 {
    if fahrenheit {
        celsius * 9.0 / 5.0 + 32.0
    } else {
        celsius
    }
}

/// Converted temperature rounded to one decimal — chart series values that
/// land verbatim in tooltips.
pub(crate) fn convert_temp_1dp(celsius: f64, fahrenheit: bool) -> f64 {
    (convert_temp(celsius, fahrenheit) * 10.0).round() / 10.0
}

/// Displayed temperature honoring the °C/°F preference; the wire is metric.
pub(crate) fn format_temp(celsius: f64, fahrenheit: bool) -> String {
    format!("{:.0}°", convert_temp(celsius, fahrenheit))
}

/// The "lead · feels X° · wind Y km/h · Z% humidity" line shared by the
/// dashboard weather widget (lead = location) and the weather page rows
/// (lead = description).
pub(crate) fn weather_details(
    lead: &str,
    weather: &chaos_domain::WeatherData,
    fahrenheit: bool,
) -> String {
    format!(
        "{} · feels {} · wind {:.0} km/h{}",
        lead,
        format_temp(weather.apparent_c, fahrenheit),
        weather.wind_kmh,
        weather
            .humidity_pct
            .map(|h| format!(" · {h:.0}% humidity"))
            .unwrap_or_default(),
    )
}
```

And append this test module at the end of lib.rs:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn sample_weather() -> chaos_domain::WeatherData {
        chaos_domain::WeatherData {
            location: "Paris, FR".into(),
            temperature_c: 21.0,
            apparent_c: 19.6,
            humidity_pct: Some(55.0),
            wind_kmh: 12.3,
            weather_code: 1,
            description: "Mainly clear".into(),
            daily: Vec::new(),
            hourly: Vec::new(),
            now_index: 0,
        }
    }

    #[test]
    fn convert_temp_handles_both_units() {
        assert_eq!(convert_temp(0.0, false), 0.0);
        assert_eq!(convert_temp(0.0, true), 32.0);
        assert_eq!(convert_temp(100.0, true), 212.0);
        assert_eq!(convert_temp_1dp(21.34, false), 21.3);
        assert_eq!(convert_temp_1dp(21.34, true), 70.4);
        assert_eq!(format_temp(19.6, false), "20°");
        assert_eq!(format_temp(19.6, true), "67°");
    }

    #[test]
    fn weather_details_joins_the_parts() {
        let weather = sample_weather();
        assert_eq!(
            weather_details("Paris, FR", &weather, false),
            "Paris, FR · feels 20° · wind 12 km/h · 55% humidity"
        );
        let mut weather = weather;
        weather.humidity_pct = None;
        assert_eq!(
            weather_details("Mainly clear", &weather, true),
            "Mainly clear · feels 67° · wind 12 km/h"
        );
    }
}
```

(If `WeatherData` has gained/lost fields since this plan was written, adjust `sample_weather()` — the asserted strings only depend on `apparent_c`, `wind_kmh`, `humidity_pct`.)

Run: `cargo test -p chaos-ui convert_temp -- --nocapture && cargo test -p chaos-ui weather_details`
Expected: PASS — both new tests.

- [ ] **Step 4.4: (c) migrate the three conversion sites + two details sites**

1. `pages/home.rs`, in `chart_option`: replace the `convert` closure

```rust
    let convert = move |celsius: f64| {
        let value = if fahrenheit {
            celsius * 9.0 / 5.0 + 32.0
        } else {
            celsius
        };
        // One decimal: these values land verbatim in the tooltip.
        (value * 10.0).round() / 10.0
    };
```

with:

```rust
    let convert = move |celsius: f64| crate::convert_temp_1dp(celsius, fahrenheit);
```

2. `pages/weather.rs`, `hourly_temps`: replace the map body so the function reads:

```rust
fn hourly_temps(hourly: &[chaos_domain::HourlyForecast], fahrenheit: bool) -> Vec<f64> {
    hourly
        .iter()
        .map(|h| crate::convert_temp_1dp(h.temp_c, fahrenheit))
        .collect()
}
```

3. `pages/dashboard.rs`, `WeatherView`: replace the `let details = format!( … )` block with:

```rust
    let details = crate::weather_details(&weather.location, &weather, fahrenheit);
```

4. `pages/weather.rs`, `weather_row_body`: replace its `let details = format!( … )` block with:

```rust
    let details = crate::weather_details(&weather.description, &weather, fahrenheit);
```

Run: `cargo test -p chaos-ui` — Expected: PASS, including the pre-existing tests in `pages/weather.rs` (they cover `hourly_temps`/`y_range` behavior through the migration).

- [ ] **Step 4.5: (d) `use_logout()` + (e) ServerGate reuses `set_api_base_override`**

(d) In `crates/chaos-ui/src/lib.rs`, add next to `use_session`:

```rust
/// Sign-out shared by the topbar and the More page: server-side logout,
/// stored token cleared, session signal reset.
pub(crate) fn use_logout() -> Callback<leptos::ev::MouseEvent> {
    let session = use_session();
    let client = use_client();
    Callback::new(move |_: leptos::ev::MouseEvent| {
        let client = client.clone();
        spawn_local(async move {
            let _ = client.logout().await;
            store_token(None);
            session.0.set(None);
        });
    })
}
```

Then in `App` (lib.rs) replace the whole `let logout = Callback::new({ … });` block with `let logout = use_logout();` (it runs after `provide_context(session)`, so `use_session()` resolves). In `pages/more.rs` replace the `let logout = Callback::new(move |_| { … });` block with `let logout = crate::use_logout();` and drop the now-unused `use leptos::task::spawn_local;` and `store_token`/`use_client` imports (keep `use_session` — the account block still reads it; let the compiler confirm which imports remain).

(e) In lib.rs's `ServerGate`, the `connect` closure re-implements `set_api_base_override` by hand. Replace

```rust
        if let Some(window) = web_sys::window() {
            if let Ok(Some(storage)) = window.local_storage() {
                let _ = storage.set_item(API_BASE_KEY, &value);
            }
            let _ = window.location().reload();
        }
```

with:

```rust
        set_api_base_override(Some(&value));
```

(`set_api_base_override` stores the value and reloads — identical observable behavior.)

Run: `cargo test -p chaos-ui && cargo check -p chaos-ui` — Expected: PASS, no unused-import warnings. Commit (c)+(d)+(e):

```bash
git add crates/chaos-ui/src/lib.rs crates/chaos-ui/src/pages/dashboard.rs crates/chaos-ui/src/pages/home.rs crates/chaos-ui/src/pages/weather.rs crates/chaos-ui/src/pages/more.rs
git commit -m "$(cat <<'EOF'
refactor(ui): dedupe temp conversion, weather details, logout, gate

convert_temp/convert_temp_1dp/weather_details replace three inlined
Fahrenheit conversions and two details strings; use_logout replaces the
duplicated topbar/More sign-out flow; ServerGate calls the existing
set_api_base_override instead of re-implementing it.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

- [ ] **Step 4.6: (f) move handlers out of api/mod.rs; CalendarEvent constructor**

**Handlers.** `crates/chaos-server/src/api/mod.rs` currently holds, below the router: `health`, `locale_fahrenheit`, `dashboard`, `WidgetQuery`, `widget_data`, `weather`, `widget_systemd`, `service_systemd`, `on_demand_service`, `services`, and the `tests` module testing `on_demand_service`. Move them verbatim (code unchanged except visibility and imports):

1. Create `crates/chaos-server/src/api/services.rs` containing: the module doc `//! Health and monitored-service endpoints.`, then `health`, `locale_fahrenheit`, `services`, `service_systemd`, `on_demand_service`, and the existing `#[cfg(test)] mod tests` (it tests `on_demand_service`). Make the three handlers and `health` `pub async fn`; `locale_fahrenheit` and `on_demand_service` stay private. Imports needed (copy from mod.rs and prune to what compiles):

```rust
use axum::Json;
use axum::extract::{Path, State};
use chaos_domain::{
    HealthResponse, ServiceActionRequest, ServiceDef, ServiceStatus, ServiceWithStatus,
};

use crate::api::ApiError;
use crate::state::AppState;
```

2. Create `crates/chaos-server/src/api/widgets.rs` containing: the module doc `//! Dashboard layout, widget payload and weather endpoints.`, then `dashboard`, `WidgetQuery`, `widget_data`, `weather`, `widget_systemd`, all handlers `pub async fn` (`WidgetQuery` stays private). Imports:

```rust
use axum::Json;
use axum::extract::{Path, Query, State};
use chaos_domain::{DashboardLayout, SystemdActionRequest, WidgetData};

use crate::api::ApiError;
use crate::state::AppState;
```

3. In `api/mod.rs`: add `mod services;` and `mod widgets;` to the module list; delete everything below the `router` function except `pub use error::ApiError;`; update the routes:
   - `get(health)` → `get(services::health)`
   - `get(services)` → `get(services::services)`
   - `post(service_systemd)` → `post(services::service_systemd)`
   - `get(dashboard)` → `get(widgets::dashboard)`
   - `get(widget_data)` → `get(widgets::widget_data)`
   - `post(widget_systemd)` → `post(widgets::widget_systemd)`
   - `get(weather)` → `get(widgets::weather)`
   Prune mod.rs's imports down to what the router still needs (roughly: `axum::routing::{get, post, put}`, `axum::Router`, the tower_http items, `crate::state::AppState`).

**CalendarEvent constructor.** In `crates/chaos-server/src/api/calendar.rs`, the `events` handler builds `CalendarEvent` twice with identical field-by-field mapping (once from db `(event, calendar_name, color)` tuples with `id: Some(...)`, once from feed events with `id: None`). Add above `events`:

```rust
/// One wire event; local events carry their id, feed occurrences don't
/// (feeds are read-only, there is nothing to address).
fn calendar_event(
    id: Option<Uuid>,
    calendar_id: Uuid,
    calendar_name: String,
    color: Option<String>,
    title: String,
    description: Option<String>,
    location: Option<String>,
    starts_at: chrono::DateTime<chrono::Utc>,
    ends_at: chrono::DateTime<chrono::Utc>,
    all_day: bool,
) -> CalendarEvent {
    CalendarEvent {
        id,
        calendar_id,
        calendar_name,
        color,
        title,
        description,
        location,
        starts_at,
        ends_at,
        all_day,
    }
}
```

and replace the two struct literals:

```rust
        .map(|(event, calendar_name, color)| {
            calendar_event(
                Some(event.id),
                event.calendar_id,
                calendar_name,
                color,
                event.title,
                event.description,
                event.location,
                event.starts_at,
                event.ends_at,
                event.all_day,
            )
        })
```

```rust
            Ok(feed_events) => out.extend(feed_events.into_iter().map(|event| {
                calendar_event(
                    None,
                    calendar.id,
                    calendar.name.clone(),
                    calendar.color.clone(),
                    event.title,
                    event.description,
                    event.location,
                    event.starts_at,
                    event.ends_at,
                    event.all_day,
                )
            })),
```

(If a positional constructor feels too wide, an equally acceptable shape is two associated helpers — `CalendarEvent from a db tuple` / `from a FeedEvent + &Calendar` — as free functions `local_event(event, calendar_name, color)` and `feed_event(occurrence, calendar)` in calendar.rs; pick one, don't do both. The positional version above is the one this plan specifies.)

Run: `cargo test -p chaos-server` — Expected: PASS, including the moved `service_actions_require_a_configured_unit` test now under `api::services::tests`. Commit:

```bash
git add crates/chaos-server/src/api/mod.rs crates/chaos-server/src/api/services.rs crates/chaos-server/src/api/widgets.rs crates/chaos-server/src/api/calendar.rs
git commit -m "$(cat <<'EOF'
refactor(server): api/mod.rs is pure routing; dedupe CalendarEvent build

Handlers move to api/services.rs and api/widgets.rs unchanged; the two
field-by-field CalendarEvent constructions in the events handler share
one constructor.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

- [ ] **Step 4.7: (g) importer stops calling list_collections per collection**

In `crates/chaos-server/src/import.rs`, pass 2 currently calls `state.db.list_collections()` inside the per-collection loop (O(n²) and a needless full-table read per iteration) just to re-find the collection it created in pass 1. Reuse the pass-1 results instead:

1. Add `Collection` to the domain import: `use chaos_domain::{Collection, CollectionRequest, CreateLinkRequest};`.
2. Change the map to hold the created collections: `let mut id_map: HashMap<i64, Collection> = HashMap::new();` and in pass 1 replace `id_map.insert(old_id, created.id);` with `id_map.insert(old_id, created.clone());` (the `created.id` uses for `collection_id: Some(created.id)` and the insert-guard stay as they are; clone before the final move if the compiler complains about use-after-move — inserting the clone and keeping `created` for the links loop is the intended order).
3. Replace pass 2 in full:

```rust
    // Pass 2: restore the collection hierarchy.
    for c in &backup.collections {
        let (Some(old_id), Some(old_parent)) = (c.id, c.parent_id) else {
            continue;
        };
        let (Some(current), Some(parent)) = (id_map.get(&old_id), id_map.get(&old_parent)) else {
            continue;
        };
        state
            .db
            .update_collection(
                current.id,
                &CollectionRequest {
                    name: current.name.clone(),
                    description: current.description.clone(),
                    color: current.color.clone(),
                    parent_id: Some(parent.id),
                },
            )
            .await?;
    }
```

(`create_collection` returns the stored row, so `current` is exactly what `list_collections` used to re-fetch; the `context("imported collection vanished")` case disappears because the map lookup already guards it. `anyhow::Context` may become unused-import — check; it is still used by the file-read at the top, so it stays.)

Run: `cargo test -p chaos-server && cargo build -p chaos-server` — Expected: PASS / clean build (the importer has no unit tests; the compile plus the unchanged `create_collection`/`update_collection` db tests are the coverage). Commit:

```bash
git add crates/chaos-server/src/import.rs
git commit -m "$(cat <<'EOF'
refactor(server): reuse pass-1 collections in linkwarden import pass 2

Drops the O(n^2) list_collections() call per imported collection; the
id map now carries the created Collection rows themselves.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_012kE9Y2kUpssDZnMaYjBLRP
EOF
)"
```

- [ ] **Step 4.8: Final full verification**

Run: `cargo test --workspace && cargo check -p chaos-ui`
Expected: everything PASS, no warnings introduced by this plan. Optionally, if the wasm target is installed (it is in the dev flake): `cargo check -p chaos-ui --target wasm32-unknown-unknown` — clean.
