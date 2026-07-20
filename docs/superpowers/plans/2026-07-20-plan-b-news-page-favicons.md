# Plan B — News Page + Favicons Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A dedicated `/news` page (everywhere) with HN/Lobsters sub-tabs and 24h/48h/Week ranges, rows carrying source favicons that link to the article, with the two widgets removed from the phone dashboard.

**Architecture:** A new server route `GET /api/v1/posts/{source}` serves `PostsData` independent of any configured widget, reusing `widgets::posts`. `chaos-client` gets `posts_list(source)`. A new `pages/news.rs` page owns persisted `source`/`range` signals and renders `post_row_view` rows (favicons via the existing `fav:` icon proxy). `FeedItem` gains an `id` so rows can later link to the reader route. Phone drops the widgets; desktop keeps them (now via `PostsBody`/`post_row_view`).

**Tech Stack:** Leptos 0.8 CSR (`chaos-ui`), Axum (`chaos-server`), `chaos-client`, `chaos-domain`. Depends on: Plan A merged. Spec: `docs/superpowers/specs/2026-07-20-news-tab-reader-design.md`.

**Verification commands (every task):**
- `cargo test -p <crate touched>` (and `-p chaos-ui` for UI tasks)
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`
- wasm check (UI tasks): `cargo check -p chaos-ui --target wasm32-unknown-unknown`

---

### Task B0: `FeedItem.id` + `Source` enum (domain)

**Files:**
- Modify: `crates/chaos-domain/src/dashboard.rs` (`FeedItem`, new `Source`)
- Test: same file `mod tests`

- [ ] **Step 1: Write failing tests.**

```rust
#[test]
fn source_round_trips() {
    assert_eq!(Source::from_str("hackernews"), Some(Source::HackerNews));
    assert_eq!(Source::from_str("lobsters"), Some(Source::Lobsters));
    assert_eq!(Source::from_str("nope"), None);
    assert_eq!(Source::HackerNews.as_str(), "hackernews");
    assert_eq!(Source::Lobsters.as_str(), "lobsters");
}

#[test]
fn feed_item_id_defaults_none_in_json() {
    // Existing wire payloads without `id` still deserialize.
    let json = r#"{"title":"t","url":null,"source":null,"published":null,
        "score":null,"comments":null,"comments_url":null}"#;
    let item: FeedItem = serde_json::from_str(json).unwrap();
    assert_eq!(item.id, None);
}
```

- [ ] **Step 2: Run tests, verify they fail.** Run: `cargo test -p chaos-domain source_ feed_item_id -v`. Expected: FAIL (no `Source`, no `id`).

- [ ] **Step 3: Add `id` to `FeedItem` (serde default) and the `Source` enum.**

```rust
pub struct FeedItem {
    pub title: String,
    pub url: Option<Url>,
    pub source: Option<String>,
    pub published: Option<DateTime<Utc>>,
    pub score: Option<u64>,
    pub comments: Option<u64>,
    pub comments_url: Option<Url>,
    /// Provider id for the discussion (HN objectId, lobsters short_id).
    /// `None` for RSS/releases rows. Enables linking to the reader.
    #[serde(default)]
    pub id: Option<String>,
}

/// A posts provider with a comment-reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    HackerNews,
    Lobsters,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::HackerNews => "hackernews",
            Source::Lobsters => "lobsters",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "hackernews" => Some(Source::HackerNews),
            "lobsters" => Some(Source::Lobsters),
            _ => None,
        }
    }
}
```

> `#[serde(rename_all = "lowercase")]` serializes the enum as `"hackernews"`/
> `"lobsters"`; keep it consistent with `as_str`.

- [ ] **Step 4: Fix all `FeedItem` construction sites** to set `id`. Search:
`rg "FeedItem \{" crates/`. RSS/releases parsers set `id: None`. The posts parsers
(server `widgets/posts.rs`, client `posts.rs`) set `id: Some(objectId/short_id)` —
covered concretely in B1/B2 below, but update any construction that fails to
compile now with `id: None` and let B1/B2 populate the real ids.

- [ ] **Step 5: Run tests + `cargo build --workspace`.** Expected: green.

- [ ] **Step 6: Commit.**

```bash
git add crates/chaos-domain/src/dashboard.rs
git -c commit.gpgsign=false commit -m "feat(domain): FeedItem.id + Source enum for the news reader"
```

---

### Task B1: Populate `id` + `/api/v1/posts/{source}` (server)

**Files:**
- Modify: `crates/chaos-server/src/widgets/posts.rs` (`algolia_item`/lobsters map set `id`)
- Modify: `crates/chaos-server/src/api/mod.rs` (route)
- Modify: `crates/chaos-server/src/api/widgets.rs` (or new `api/posts.rs`) (handler)
- Modify: `crates/chaos-server/src/widgets/mod.rs` (a `posts_list(source)` method on `WidgetService`)

- [ ] **Step 1: Set `id` in the server posts mappers.** In `widgets/posts.rs`,
`algolia_item` sets `id: Some(hit.object_id.clone())`; the lobsters mapper sets
`id: Some(story.short_id.clone())`. Add a unit test asserting a mapped HN hit and a
mapped lobsters story carry a non-empty `id`.

- [ ] **Step 2: Add `NEWS_LIMIT` + a `posts_list` method** on `WidgetService`
(`widgets/mod.rs`), cached like widgets but keyed by source:

```rust
pub const NEWS_LIMIT: u32 = 50;

pub async fn posts_list(&self, source: chaos_domain::Source) -> Result<WidgetData, WidgetError> {
    use chaos_domain::Source;
    let key = format!("posts:{}", source.as_str());
    let ttl = std::time::Duration::from_secs(300);
    let fut = async {
        match source {
            Source::HackerNews => posts::hacker_news(&self.http, NEWS_LIMIT, chrono::Utc::now()).await,
            Source::Lobsters => posts::lobsters(&self.http, NEWS_LIMIT, chrono::Utc::now()).await,
        }
    };
    self.cached_fetch(key, ttl, fut).await
}
```

> Match the real signature/return type of `cached_fetch` and `WidgetError` in
> `widgets/mod.rs`; the existing `data(id, widget)` shows the exact shape.

- [ ] **Step 3: Add the handler + route.** In `api/widgets.rs` (or a new
`api/posts.rs` module wired in `api/mod.rs`):

```rust
pub async fn posts_list(
    State(state): State<AppState>,
    Path(source): Path<String>,
) -> Result<Json<WidgetData>, ApiError> {
    let source = chaos_domain::Source::from_str(&source)
        .ok_or(ApiError::NotFound)?;
    let data = state.widgets.posts_list(source).await
        .map_err(ApiError::from)?;
    Ok(Json(data))
}
```

Route in `api/mod.rs` (near the widgets routes ~line 45):

```rust
.route("/posts/{source}", get(widgets::posts_list))
```

> Match `AppState`/`ApiError`/state field names to the codebase (see existing
> `widgets::widget_data`). Unknown source → 404; upstream failure → whatever
> `cached_fetch` returns (stale or 502), same as widgets.

- [ ] **Step 4: Test the endpoint.** Add a server test (follow the existing widget
handler test pattern; if the suite mocks upstream, mirror it — otherwise assert the
router returns 404 for an unknown source):

```rust
#[tokio::test]
async fn posts_list_unknown_source_is_404() {
    // build the router/app as other api tests do, then:
    // GET /api/v1/posts/nope -> 404
}
```

- [ ] **Step 5: Run `cargo test -p chaos-server` + clippy + fmt.** Expected: green.

- [ ] **Step 6: Commit.**

```bash
git add crates/chaos-server
git -c commit.gpgsign=false commit -m "feat(server): GET /api/v1/posts/{source} + populate FeedItem.id"
```

---

### Task B2: `posts_list` client + populate direct `id`

**Files:**
- Modify: `crates/chaos-client/src/lib.rs` (`posts_list` typed call)
- Modify: `crates/chaos-client/src/posts.rs` (direct parsers set `id`)

- [ ] **Step 1: Set `id` in the client/direct parsers.** In `chaos-client/src/posts.rs`,
`algolia_item` sets `id: Some(hit.object_id.clone())`; `parse_lobsters_page` sets
`id: Some(story.short_id.clone())`. Add unit tests over a fixture JSON asserting the
parsed items carry a non-empty `id`.

- [ ] **Step 2: Write a failing test for `posts_list` route building** (the client
mirrors other typed calls). If the client has URL-building tests, assert
`posts_list` targets `api/v1/posts/hackernews`. Otherwise add:

```rust
#[test]
fn posts_list_url() {
    let c = Client::new_for_test("http://x/"); // use the test constructor the crate provides
    assert_eq!(c.url("api/v1/posts/hackernews").unwrap().as_str(),
               "http://x/api/v1/posts/hackernews");
}
```

- [ ] **Step 3: Add the typed call** in `chaos-client/src/lib.rs`, next to
`widget_data`:

```rust
pub async fn posts_list(&self, source: chaos_domain::Source) -> Result<WidgetData> {
    self.get(&format!("api/v1/posts/{}", source.as_str())).await
}
```

- [ ] **Step 4: Run `cargo test -p chaos-client` + clippy.** Expected: green.

- [ ] **Step 5: Commit.**

```bash
git add crates/chaos-client
git -c commit.gpgsign=false commit -m "feat(client): posts_list(source) + populate direct FeedItem.id"
```

---

### Task B3: `Source` UI plumbing + persisted prefs

**Files:**
- Modify: `crates/chaos-ui/src/lib.rs` (pref keys + helpers)

- [ ] **Step 1: Write failing tests** in `lib.rs` `mod tests` (or wherever prefs
are tested). Since `pref` hits localStorage (browser-only), test the pure default
logic by asserting the key constants and that the parse helper maps strings:

```rust
#[test]
fn news_source_parses() {
    assert_eq!(news_source_from(Some("lobsters")), chaos_domain::Source::Lobsters);
    assert_eq!(news_source_from(None), chaos_domain::Source::HackerNews); // default
    assert_eq!(news_source_from(Some("garbage")), chaos_domain::Source::HackerNews);
}
```

- [ ] **Step 2: Run, verify fail.** Run: `cargo test -p chaos-ui news_source -v`. Expected: FAIL.

- [ ] **Step 3: Add keys + helpers** mirroring `weather_combined`:

```rust
pub(crate) const NEWS_SOURCE_KEY: &str = "chaos-news-source";
pub(crate) const NEWS_RANGE_KEY: &str = "chaos-news-range";

fn news_source_from(raw: Option<&str>) -> chaos_domain::Source {
    raw.and_then(chaos_domain::Source::from_str).unwrap_or(chaos_domain::Source::HackerNews)
}
pub(crate) fn news_source() -> chaos_domain::Source {
    news_source_from(pref(NEWS_SOURCE_KEY).as_deref())
}
pub(crate) fn set_news_source(s: chaos_domain::Source) {
    set_pref(NEWS_SOURCE_KEY, s.as_str());
}
pub(crate) fn news_range() -> u8 {
    // 0=24h, 1=48h, 2=week; stored as "0"/"1"/"2".
    pref(NEWS_RANGE_KEY).and_then(|v| v.parse().ok()).filter(|n| *n <= 2).unwrap_or(0)
}
pub(crate) fn set_news_range(idx: u8) {
    set_pref(NEWS_RANGE_KEY, &idx.to_string());
}
```

> `news_range` returns an index the page maps to `PostsTab`; keeping it a small int
> avoids importing `PostsTab` into `lib.rs`. If `PostsTab` is public and importable,
> store/return `PostsTab` directly instead — pick whichever keeps `lib.rs` clean.

- [ ] **Step 4: Run tests + clippy.** Expected: green.

- [ ] **Step 5: Commit.**

```bash
git add crates/chaos-ui/src/lib.rs
git -c commit.gpgsign=false commit -m "feat(ui): persisted news source/range device prefs"
```

---

### Task B4: `post_row_view` with favicon

**Files:**
- Modify: `crates/chaos-ui/src/pages/dashboard.rs` (new `post_row_view`; `PostsBody` uses it; import `Source`)
- Modify: `crates/chaos-web/styles.css` (`.post-row`, `.post-favicon`)

- [ ] **Step 1: Write a failing test** for the favicon host/spec logic (pure):

```rust
#[test]
fn favicon_spec_uses_article_host() {
    let item = FeedItem { title: "t".into(),
        url: Some("https://example.com/a".parse().unwrap()),
        source: Some("Hacker News".into()), published: None, score: Some(5),
        comments: None, comments_url: None, id: Some("1".into()) };
    assert_eq!(favicon_spec(&item, Source::HackerNews), "fav:example.com");
}

#[test]
fn favicon_spec_falls_back_to_source_host() {
    let item = FeedItem { title: "Ask HN".into(), url: None,
        source: Some("Hacker News".into()), published: None, score: Some(5),
        comments: None, comments_url: None, id: Some("1".into()) };
    assert_eq!(favicon_spec(&item, Source::HackerNews), "fav:news.ycombinator.com");
    let lob = FeedItem { url: None, ..item };
    assert_eq!(favicon_spec(&lob, Source::Lobsters), "fav:lobste.rs");
}
```

- [ ] **Step 2: Run, verify fail.** Run: `cargo test -p chaos-ui favicon_spec -v`. Expected: FAIL.

- [ ] **Step 3: Add `favicon_spec` + `post_row_view`.**

```rust
fn favicon_spec(item: &FeedItem, source: Source) -> String {
    let host = item.url.as_ref().and_then(|u| u.host_str().map(str::to_owned));
    let host = host.unwrap_or_else(|| match source {
        Source::HackerNews => "news.ycombinator.com".into(),
        Source::Lobsters => "lobste.rs".into(),
    });
    format!("fav:{host}")
}

/// A rich posts row: title links to the in-app reader, favicon (right) links
/// to the article. `client` is used to resolve the cached favicon proxy URL.
fn post_row_view(item: FeedItem, anchor: Option<u64>, source: Source, client: Client) -> impl IntoView {
    let spec = favicon_spec(&item, source);
    let fav_url = client.icon_url(&spec).map(|u| u.to_string()).unwrap_or_default();
    // favicon links to the article, or the discussion when there is no article.
    let fav_href = item.url.as_ref().map(|u| u.to_string())
        .or_else(|| item.comments_url.as_ref().map(|u| u.to_string()))
        .unwrap_or_default();
    let reader_href = item.id.as_ref()
        .map(|id| format!("/news/{}/{}", source.as_str(), id))
        .unwrap_or_default();
    let score = item.score.map(|s| format!("▲ {s}"));
    let score_style = match (item.score, anchor) {
        (Some(s), Some(a)) => Some(score_color(s, a)),
        _ => None,
    };
    let comments = item.comments.map(|n| format!("{n} comment{}", if n == 1 { "" } else { "s" }));
    let age = item.published.map(rel_time);
    let title = item.title.clone();
    view! {
        <li class="post-row">
            <div class="post-main">
                <a class="post-title" href=reader_href>{title}</a>
                <span class="muted feed-meta">
                    <span class="feed-score" style:color=score_style>{score}</span>
                    <span class="feed-comments">{comments}</span>
                    <span class="feed-age">{age}</span>
                </span>
            </div>
            <a class="post-favicon" href=fav_href target="_blank" rel="noreferrer">
                <img src=fav_url alt="" loading="lazy"
                     on:error=|ev| {
                        // hide broken favicons; CSS shows the fallback glyph
                        let _ = ev; // set display:none via class toggle in impl
                     }/>
            </a>
        </li>
    }
}
```

> The `on:error` handler must hide the broken `<img>` (e.g. add a `hidden` class or
> set `display:none` on the target). Implement it concretely using
> `web_sys`/`event_target` as the codebase does elsewhere; the `.post-favicon`
> anchor keeps a CSS `::after` fallback glyph so an empty/broken icon still shows a
> neutral mark.

- [ ] **Step 4: Point `PostsBody` at `post_row_view`.** `PostsBody` needs the
`Source` and a `Client`. Thread them in: the Posts match arm already knows the
`widget` (`Widget::HackerNews`/`Lobsters`) — map it to `Source` and pass
`Source` + the `Client` (available via context `expect_context::<Client>()` or the
existing accessor) into `PostsBody`, which forwards them to `post_row_view`. Replace
`feed_item_view(item, anchor)` inside `PostsBody` with
`post_row_view(item, anchor, source, client.clone())`.

- [ ] **Step 5: Add CSS.** In `styles.css`, near `.posts-tabs`:

```css
.post-row { display: flex; align-items: flex-start; gap: .6rem; }
.post-row .post-main { flex: 1 1 auto; min-width: 0; }
.post-favicon { flex: 0 0 auto; width: 20px; height: 20px; display: inline-flex;
    align-items: center; justify-content: center; }
.post-favicon img { width: 16px; height: 16px; border-radius: 3px; }
.post-favicon img.hidden { display: none; }
.post-favicon:has(img.hidden)::after,
.post-favicon:not(:has(img))::after { content: "🔗"; font-size: 13px; opacity: .5; }
```

- [ ] **Step 6: Run all verification commands.** Expected: green.

- [ ] **Step 7: Commit.**

```bash
git add crates/chaos-ui crates/chaos-web/styles.css
git -c commit.gpgsign=false commit -m "feat(posts): favicon rows linking to article + in-app reader"
```

---

### Task B5: `/news` page + navigation + phone dashboard drop

**Files:**
- Create: `crates/chaos-ui/src/pages/news.rs`
- Modify: `crates/chaos-ui/src/pages/mod.rs` (export `NewsPage`)
- Modify: `crates/chaos-ui/src/lib.rs` (`Route`, `NAV_PRIMARY`, desktop topbar)
- Modify: `crates/chaos-ui/src/pages/dashboard.rs` (phone widget skip)
- Modify: `crates/chaos-web/styles.css` (`.news-sources`)

- [ ] **Step 1: Create `NewsPage`.** It owns persisted `source`/`range`, loads
`PostsData` via `client.posts_list(source)` online with offline direct fallback
(reuse the dashboard's `DirectFeed`/`cached`/`cached_direct` mechanism — extract a
shared `load_posts(source, conn, client)` helper if the dashboard's is not reusable
as-is), and renders `post_row_view` rows under one anchor over the three windows.

```rust
#[component]
pub fn NewsPage() -> impl IntoView {
    let client = expect_context::<Client>();
    let conn = expect_context::<crate::offline::ConnSignal>(); // match the real type
    let source = RwSignal::new(crate::news_source());
    let range = RwSignal::new(crate::news_range()); // u8 index → PostsTab
    // persist on change
    Effect::new(move |_| crate::set_news_source(source.get()));
    Effect::new(move |_| crate::set_news_range(range.get()));

    let data = LocalResource::new(move || {
        let (client, conn, source) = (client.clone(), conn, source.get());
        async move { load_posts(source, conn, &client).await } // -> Result<PostsData, String>
    });

    view! {
        <section class="news-page">
            <div class="news-sources">
                {[(Source::HackerNews, "Hacker News"), (Source::Lobsters, "lobste.rs")]
                    .map(|(s, label)| view! {
                        <button class:active=move || source.get() == s
                                on:click=move |_| source.set(s)>{label}</button>
                    })}
            </div>
            // range tabs (24h/48h/Week) driven by `range`, same markup as `.posts-tabs`
            // list: Suspend over `data`, choose window by `range`, color by union anchor,
            //       render `post_row_view(item, anchor, source.get(), client.clone())`.
        </section>
    }
}
```

> Fill the range strip and the `Suspend`/`Transition` list body concretely using the
> dashboard's existing patterns. The list body must select the window via a
> top-level `Memo` keyed on `range` (same discipline as Plan A) so range clicks
> switch. The union anchor is `score_anchor(all three windows)`.

- [ ] **Step 2: Register the page.** `pages/mod.rs`: `mod news; pub use news::NewsPage;`.
`lib.rs` `<Routes>`: add `<Route path=path!("/news") view=pages::NewsPage/>`.

- [ ] **Step 3: Add nav entries.** In `lib.rs`, extend `NAV_PRIMARY` to include
`("/news", "▤", "News")` and bump the array length from `5` to `6`:

```rust
const NAV_PRIMARY: [(&str, &str, &str); 6] = [
    ("/", "▦", "Dash"),
    ("/links", "⛓", "Links"),
    ("/news", "▤", "News"),
    ("/weather", "☀", "Weather"),
    ("/more", "≡", "More"),
];
```

Add `<A href="/news">"News"</A>` to the desktop `<nav class="topbar">` link set.

- [ ] **Step 4: Drop the widgets on phone.** In `dashboard.rs`, where widgets are
rendered, skip `Widget::HackerNews`/`Widget::Lobsters` at phone width. Use the app's
existing responsive signal if one exists; if the topbar/tabbar split is CSS-only,
render the posts widget wrapped in a `class="hide-on-phone"` container and add
`@media (max-width: <phone breakpoint>) { .hide-on-phone { display: none } }` to
`styles.css` (match the existing breakpoint used for topbar↔tabbar). Prefer the CSS
approach if that mirrors how the app already hides desktop-only nav.

> Determine the existing breakpoint by grepping `styles.css` for the media query
> that toggles `.topbar`/`.tabbar`; reuse that exact value.

- [ ] **Step 5: Style the source tabs.** In `styles.css`, `.news-sources` mirrors
`.posts-tabs` but a touch larger (primary switch):

```css
.news-sources { display: flex; gap: .5rem; margin-bottom: .75rem; }
.news-sources button { flex: 1; padding: .5rem; background: var(--surface);
    border: 1px solid var(--border); border-radius: 6px; color: var(--muted); }
.news-sources button.active { color: var(--accent); border-color: var(--accent); }
```

- [ ] **Step 6: Run all verification commands + trunk build.** Run:
`cargo test -p chaos-ui && cargo check -p chaos-ui --target wasm32-unknown-unknown && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check`.
Expected: green.

- [ ] **Step 7: Commit.**

```bash
git add crates/chaos-ui crates/chaos-web/styles.css
git -c commit.gpgsign=false commit -m "feat(news): dedicated /news page, nav entry, phone dashboard drops posts widgets"
```

---

## Self-review notes
- Spec coverage: B0/B1/B2 = Posts API + `id`; B3 = persisted prefs; B4 = favicon
  rows + reader links; B5 = page, nav, phone-drop. All §Plan B items covered.
- The reader route `/news/{source}/{id}` is *linked* here (B4) but only
  *implemented* in Plan C. Between B and C, tapping a title navigates to a route
  that 404s in the router until C lands — acceptable because B and C ship together
  or C follows immediately; note this in the B release notes if B ships alone
  (unlikely). If B must ship alone, add a temporary stub route in B5 rendering
  "Reader coming soon" and delete it in C.
- Type consistency: `Source` (domain), `favicon_spec(&FeedItem, Source) -> String`,
  `post_row_view(FeedItem, Option<u64>, Source, Client)`,
  `posts_list(Source) -> Result<WidgetData>`, `news_range() -> u8`. Consistent.
- New egress: none beyond existing (search/newest already allowed); favicons use the
  existing `fav:` proxy (duckduckgo).
