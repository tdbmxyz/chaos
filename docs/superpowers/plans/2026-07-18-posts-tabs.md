# HN/Lobsters Time-Range Tabs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** HN and Lobsters widgets get Last 24h / Last 48h / Last week tabs showing the true top-by-upvotes links per window, online and offline.

**Architecture:** New `WidgetData::Posts(PostsData)` payload with the three pre-computed lists. Server fetches HN from the Algolia archive API (one query per window, `tags=front_page`) and Lobsters by paginating `newest.json` back one week, bucketing by `created_at`. The client direct-fetch path (offline) mirrors both — HN Algolia sends CORS `*` so plain reqwest works everywhere; lobsters pages go through the Tauri HTTP plugin. Tabs are pure client-side UI over the cached payload. Server and chaos-client keep duplicate mapping code by policy (server must not depend on chaos-client).

**Tech Stack:** Axum server, reqwest, chrono, serde; Leptos UI. Spec: `docs/superpowers/specs/2026-07-18-weather-timeaxis-posts-tabs-design.md`.

**Verified upstream facts (curl, 2026-07-18):**
- `https://hn.algolia.com/api/v1/search?tags=front_page&numericFilters=created_at_i>{cutoff}&hitsPerPage=50` → `{"hits": [{"title", "url" (nullable), "points", "num_comments", "created_at_i", "objectID", …}]}`, header `access-control-allow-origin: *`. Relevance-ranked, so re-sort by points.
- `https://lobste.rs/newest.json?page=N` → JSON array of stories, newest first, ~25/page ≈ 1.3 days/page; same story shape as hottest.json (`title,url,score,comment_count,comments_url,created_at`). No CORS headers (Tauri plugin required in browsers).

**Verification commands:** `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`, `cargo check -p chaos-ui --target wasm32-unknown-unknown`.

**Commits:** unsigned (`git -c commit.gpgsign=false commit`).

---

### Task B1: Domain type + server fetchers

**Files:**
- Modify: `crates/chaos-domain/src/dashboard.rs`
- Modify: `crates/chaos-server/src/widgets/posts.rs` (rewrite)
- Modify: `crates/chaos-server/src/widgets/mod.rs` (call sites pass `Utc::now()` if the new signatures need it)

- [ ] **Step 1: Domain.** Add to `chaos-domain/src/dashboard.rs` next to `FeedItem`:

```rust
/// Top links of the trailing 24 h / 48 h / week, each sorted by upvotes
/// descending and truncated to the widget limit. Produced for the
/// HackerNews/Lobsters widgets (RSS `Feed` widgets keep `WidgetData::Feed`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PostsData {
    pub last_24h: Vec<FeedItem>,
    pub last_48h: Vec<FeedItem>,
    pub last_week: Vec<FeedItem>,
}
```

and a `Posts(PostsData)` variant to `WidgetData`. Fix any exhaustive matches the compiler flags (chaos-ui's DataWidget match gets its real arm in B3; give it a placeholder arm rendering nothing only if needed to keep the workspace compiling — B3 replaces it).

- [ ] **Step 2: Failing server tests** in `chaos-server/src/widgets/posts.rs`: keep/adapt the existing mapping tests (`maps_a_lobsters_story`, text-post fallback, sort order); add:

```rust
#[test]
fn algolia_hit_maps_points_comments_and_discussion() { /* parse a raw hit json with url null → discussion link; created_at_i → published */ }

#[test]
fn windowed_filters_sorts_and_truncates() {
    // items published now-1h (score 5), now-30h (score 50), now-6d (score 9),
    // now-9d (score 99), one with published: None (score 100):
    // windowed(24h)  -> [5]
    // windowed(48h)  -> [50, 5]
    // windowed(168h) -> [50, 9, 5]   (9-day-old and unpublished excluded)
    // limit 2        -> truncates to the top 2
}
```

- [ ] **Step 3: Run tests, verify the new ones fail.**

- [ ] **Step 4: Rewrite the fetchers.**

```rust
const ALGOLIA_SEARCH: &str = "https://hn.algolia.com/api/v1/search";
const LOBSTERS_NEWEST: &str = "https://lobste.rs/newest.json";
/// newest.json covers ~1.3 days per 25-story page; 10 pages safely spans a week.
const LOBSTERS_PAGE_CAP: u32 = 10;
const WEEK_HOURS: i64 = 24 * 7;

#[derive(Deserialize)]
struct AlgoliaResponse { hits: Vec<AlgoliaHit> }

#[derive(Deserialize)]
struct AlgoliaHit {
    title: Option<String>,
    url: Option<String>,
    points: Option<u64>,
    num_comments: Option<u64>,
    created_at_i: Option<i64>,
    #[serde(rename = "objectID")]
    object_id: String,
}

fn algolia_item(hit: AlgoliaHit) -> FeedItem {
    let discussion: Option<Url> = format!("{HN_ITEM}{}", hit.object_id).parse().ok();
    FeedItem {
        title: hit.title.unwrap_or_else(|| "(untitled)".into()),
        url: hit.url.filter(|u| !u.is_empty()).and_then(|u| u.parse().ok())
            .or_else(|| discussion.clone()),
        source: Some("Hacker News".into()),
        published: hit.created_at_i.and_then(|t| DateTime::from_timestamp(t, 0)),
        score: hit.points,
        comments: hit.num_comments,
        comments_url: discussion,
    }
}

/// Items published within the trailing `hours`, by upvotes, top `limit`.
fn windowed(items: &[FeedItem], now: DateTime<Utc>, hours: i64, limit: u32) -> Vec<FeedItem> {
    let cutoff = now - chrono::Duration::hours(hours);
    let mut hits: Vec<FeedItem> = items.iter()
        .filter(|i| i.published.is_some_and(|p| p >= cutoff))
        .cloned()
        .collect();
    sort_by_score(&mut hits);
    hits.truncate(limit as usize);
    hits
}

pub async fn hacker_news(http: &reqwest::Client, limit: u32, now: DateTime<Utc>) -> Result<WidgetData, String> {
    // One archive query per window: relevance-ranked top-50 among stories
    // that made the front page in the window; re-sorted by points below.
    let window = |hours: i64| async move {
        let cutoff = (now - chrono::Duration::hours(hours)).timestamp();
        let url = format!("{ALGOLIA_SEARCH}?tags=front_page&numericFilters=created_at_i>{cutoff}&hitsPerPage=50");
        let resp: AlgoliaResponse = get_json(http, &url).await.map_err(|e| format!("hn algolia: {e}"))?;
        let items: Vec<FeedItem> = resp.hits.into_iter().map(algolia_item).collect();
        Ok::<_, String>(windowed(&items, now, hours, limit))
    };
    let (last_24h, last_48h, last_week) =
        futures::try_join!(window(24), window(48), window(WEEK_HOURS))?;
    if last_week.is_empty() { return Err("no stories returned".into()); }
    Ok(WidgetData::Posts(PostsData { last_24h, last_48h, last_week }))
}

pub async fn lobsters(http: &reqwest::Client, limit: u32, now: DateTime<Utc>) -> Result<WidgetData, String> {
    let cutoff = now - chrono::Duration::hours(WEEK_HOURS);
    let mut items: Vec<FeedItem> = Vec::new();
    for page in 1..=LOBSTERS_PAGE_CAP {
        let stories: Vec<LobstersStory> = get_json(http, &format!("{LOBSTERS_NEWEST}?page={page}"))
            .await
            .map_err(|e| format!("lobsters: {e}"))?;
        if stories.is_empty() { break; }
        // Newest-first: once a page's oldest story predates the cutoff,
        // later pages are all older.
        let done = stories.last().and_then(|s| s.created_at).is_none_or(|t| t < cutoff);
        items.extend(stories.into_iter().map(lobsters_item));
        if done { break; }
    }
    if items.is_empty() { return Err("no stories returned".into()); }
    Ok(WidgetData::Posts(PostsData {
        last_24h: windowed(&items, now, 24, limit),
        last_48h: windowed(&items, now, 48, limit),
        last_week: windowed(&items, now, WEEK_HOURS, limit),
    }))
}
```

Delete the Firebase (`HnItem`, `hn_item`, topstories) and `hottest.json` code paths and their now-dead tests; update the module doc comment. Update `widgets/mod.rs` call sites to pass `chrono::Utc::now()`.

- [ ] **Step 5: Run the workspace tests + clippy + fmt.** Expected: green (chaos-ui may need the temporary match arm from Step 1).

- [ ] **Step 6: Commit** `feat(server): HN/lobsters top links per 24h/48h/week window (algolia + newest pagination)`.

---

### Task B2: chaos-client direct fetchers

**Files:**
- Modify: `crates/chaos-client/src/posts.rs` (rewrite)

- [ ] **Step 1: Failing tests** mirroring B1's (`algolia` hit mapping, `windowed` semantics, and: `parse_lobsters_page` parses a raw array; `posts_from_items` buckets into the three windows).

- [ ] **Step 2: Implement.** Same `AlgoliaHit`/`algolia_item`/`windowed` code as B1 (duplicate by policy — see module doc), plus:

```rust
/// HN window-tops via the Algolia archive API (CORS `*`, works in browsers
/// and shells alike). Same three-query shape as the server.
pub async fn hacker_news(http: &reqwest::Client, limit: u32, now: DateTime<Utc>) -> Result<PostsData, String>

/// One page of `newest.json` (fetched by the caller — browsers need the
/// Tauri HTTP plugin for lobste.rs) parsed to items.
pub fn parse_lobsters_page(json: &str) -> Result<Vec<FeedItem>, String>

/// Where page `n` of the lobsters sweep lives.
pub fn lobsters_page_url(page: u32) -> String

/// Sweep stop test: newest-first pages — stop once the oldest story on the
/// page predates now-7d (or has no date).
pub fn lobsters_page_done(items: &[FeedItem], now: DateTime<Utc>) -> bool

pub const LOBSTERS_PAGE_CAP: u32 = 10;

/// Bucket a gathered item list into the three windows.
pub fn posts_from_items(items: &[FeedItem], limit: u32, now: DateTime<Utc>) -> PostsData
```

`hacker_news` uses the existing `crate::http::http_get_json`; three sequential awaits are fine (wasm). Errors if the week list ends up empty, like the server. Old `parse_lobsters(json, limit)` and the Firebase path are deleted.

- [ ] **Step 3: Tests + clippy + fmt + wasm check.** Expected green.

- [ ] **Step 4: Commit** `feat(client): direct HN/lobsters window-top fetchers`.

---

### Task B3: UI tabs, offline direct path, capability

**Files:**
- Modify: `crates/chaos-ui/src/pages/dashboard.rs` (Posts arm + tabs, DirectFeed)
- Modify: `crates/chaos-web/styles.css` (`.posts-tabs`)
- Modify: `crates/chaos-desktop/capabilities/default.json`

- [ ] **Step 1: DirectFeed.** `DirectFeed::fetch` returns `WidgetData::Posts`:

```rust
impl DirectFeed {
    async fn fetch(self) -> Result<WidgetData, ClientError> {
        let now = chrono::Utc::now();
        let posts = match self {
            DirectFeed::HackerNews(limit) => {
                chaos_client::posts::hacker_news(&crate::weather_fetch::http(), limit, now).await
            }
            DirectFeed::Lobsters(limit) => lobsters_direct(limit, now).await,
        }
        .map_err(ClientError::Transport)?;
        Ok(WidgetData::Posts(posts))
    }
}

/// Page through newest.json via the shell's HTTP plugin (lobste.rs sends no
/// CORS headers) until the sweep covers a week or the cap.
async fn lobsters_direct(limit: u32, now: chrono::DateTime<chrono::Utc>) -> Result<chaos_domain::PostsData, String> {
    use chaos_client::posts;
    let mut items = Vec::new();
    for page in 1..=posts::LOBSTERS_PAGE_CAP {
        let json = match crate::tauri_http::fetch_text(&posts::lobsters_page_url(page)).await {
            Some(Ok(json)) => json,
            Some(Err(err)) => return Err(err),
            None => return Err("lobsters needs the app shell offline".into()),
        };
        let page_items = posts::parse_lobsters_page(&json)?;
        if page_items.is_empty() { break; }
        let done = posts::lobsters_page_done(&page_items, now);
        items.extend(page_items);
        if done { break; }
    }
    if items.is_empty() { return Err("no stories returned".into()); }
    Ok(posts::posts_from_items(&items, limit, now))
}
```

(`tauri_http::fetch_text` currently takes `&'static str` or `&str` — adjust its signature to `&str` if needed.)

- [ ] **Step 2: Tabs UI.** In `DataWidget`, next to the existing `collapsed` signal:

```rust
#[derive(Clone, Copy, PartialEq)]
enum PostsTab { Day, TwoDays, Week }
let tab = RwSignal::new(PostsTab::Day);
```

New match arm (replacing any B1 placeholder):

```rust
Some(Ok((WidgetData::Posts(posts), _))) => {
    let items = move |t: PostsTab| match t {
        PostsTab::Day => posts.last_24h.clone(),
        PostsTab::TwoDays => posts.last_48h.clone(),
        PostsTab::Week => posts.last_week.clone(),
    };
    view! {
        <div class="posts-tabs">
            {[(PostsTab::Day, "24h"), (PostsTab::TwoDays, "48h"), (PostsTab::Week, "Week")]
                .map(|(t, label)| view! {
                    <button
                        class:active=move || tab.get() == t
                        on:click=move |_| tab.set(t)
                    >{label}</button>
                })}
        </div>
        {move || {
            let items = items(tab.get());
            let count = items.len();
            view! {
                <Collapsible count collapsed>
                    <ul class="feed-list">
                        {items.into_iter().map(feed_item_view).collect_view()}
                    </ul>
                </Collapsible>
            }
        }}
    }
    .into_any()
}
```

Empty tab lists fall through to the existing `Collapsible`/list empty rendering. The `WidgetData::Feed` arm stays for RSS widgets.

- [ ] **Step 3: CSS.** `.posts-tabs` in `chaos-web/styles.css`, matching existing button/pill idiom (real vars: `--border`, `--surface`, `--accent`, `--muted`): small inline row under the widget title, active tab accent-colored.

- [ ] **Step 4: Capability.** In `crates/chaos-desktop/capabilities/default.json` http scope: add `https://hn.algolia.com/*`, remove `https://hacker-news.firebaseio.com/*` (no longer called anywhere). Run a desktop `cargo check -p chaos-desktop` so the gen/schemas for desktop regenerate; android/mobile schemas regenerate on the next android build (leave them).

- [ ] **Step 5: Full verification suite.** All four commands green.

- [ ] **Step 6: Commit** `feat(ui): 24h/48h/week tabs on HN and lobsters widgets, offline included`.

---

### Task B4: Docs

**Files:**
- Modify: `docs/ROADMAP.md` (mark/record the feature)
- Modify: `docs/deployment.md` (upstream egress note: hn.algolia.com + lobste.rs/newest replace firebaseio/hottest; weather page renders viewer-local)

- [ ] **Step 1: Update both docs** per the spec's Docs section, following each file's existing tone.
- [ ] **Step 2: Commit** `docs: posts tabs + weather time-axis notes`.
