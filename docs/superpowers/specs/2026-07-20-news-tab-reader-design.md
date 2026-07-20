# News Tab, Comment Reader, Favicons & Score-Scale Design

**Date:** 2026-07-20
**Status:** Approved (design). Implementation split into three plans (A, B, C).

## Problem

The HackerNews and Lobsters "posts" widgets on the dashboard have four issues,
surfaced from on-device phone use (screenshots 2026-07-20):

1. **The 24h/48h/Week time-range tabs don't switch the list.** Tapping a tab
   moves the highlight but the rows never change.
2. **Scores clump in yellow.** The linear `score/anchor` heat scale maps almost
   every real-world score to indistinguishable yellows because the top item
   (e.g. 2146) dwarfs the rest (181–865).
3. **Posts are cramped as dashboard widgets** on phone and there is no way to
   read a discussion in-app — tapping only opens the external article.
4. **No favicons** to identify a story's source site at a glance.

## Decisions (from brainstorming, 2026-07-20)

- **Platform scope:** a dedicated `/news` page **everywhere**. On phone the
  HackerNews/Lobsters widgets are **removed from the dashboard**; on desktop the
  dashboard **keeps** them as compact glances. The News page is the rich reader
  on both.
- **Comments data:** a **server endpoint** (`GET /api/v1/posts/{source}/{id}/comments`),
  cached like other widgets, with an **offline direct-fetch fallback** mirroring
  the existing widget pattern.
- **Color scale:** **logarithmic** — `t = ln(1+score)/ln(1+anchor)`.
- **Reader UI:** a **dedicated route** `/news/{source}/{id}` with back navigation.

## Build order

Three plans, each producing a releasable app. **A ships first.**

- **Plan A — Fixes:** tab-subscription bug + log color scale. No new surface.
- **Plan B — News page + favicons:** `/news` route, HN/Lobsters sub-tabs,
  24h/48h/Week range, favicon rows, phone dashboard drops the widgets.
- **Plan C — Comment reader:** thread data + endpoint + offline fallback +
  `/news/{source}/{id}` reader with a collapsible tree. Depends on B.

---

## Plan A — Fixes

### A1. Tab-subscription bug

**Root cause:** In `crates/chaos-ui/src/pages/dashboard.rs` the Posts render arm
builds the tab-dependent list inside a `move ||` closure that is *nested* inside
the outer `{move || match data.get() { … }}` branch, which is then
`.into_any()` type-erased (dashboard.rs ~275, ~331, ~346). The inner reactive
block's subscription to the `tab` signal is dropped, so `tab.set(...)` notifies
nothing and the list never re-renders. `class:active` (a direct child) still
updates, which is why the highlight moves but the list does not.

**Fix:** Extract the Posts body into a real component,
`#[component] fn PostsBody(posts: PostsData, anchor: Option<u64>)`, that owns its
`tab` signal and a **top-level** `Memo` for the shown list:

```rust
let tab = RwSignal::new(PostsTab::Day);
let shown = Memo::new(move |_| match tab.get() {
    PostsTab::Day => posts.last_24h.clone(),
    PostsTab::TwoDays => posts.last_48h.clone(),
    PostsTab::Week => posts.last_week.clone(),
});
```

Rendered from a proper owner scope, the `Memo`'s subscription survives and tabs
switch. The Posts match arm becomes `<PostsBody posts anchor/>`. This fixes the
desktop dashboard widget too (same bug).

### A2. Logarithmic color scale

In `score_color` (dashboard.rs ~840), replace the linear normalization:

```rust
// before: let t = (score as f64 / anchor as f64).clamp(0.0, 1.0);
let t = if anchor == 0 {
    0.0
} else {
    (((score as f64) + 1.0).ln() / ((anchor as f64) + 1.0).ln()).clamp(0.0, 1.0)
};
```

`anchor` is unchanged — the p99-over-the-union value from `score_anchor`, so the
three tabs still share one scale. The 5-stop `HEAT_STOPS` gradient and the
minimum-of-0 behavior are unchanged. Effect: the crowded low end expands (181 vs
497 become distinct) while the top score still reads hard red.

**Tests:** `score_color` for a clustered set — assert monotonicity and that two
mid-range scores now map to different gradient buckets where the linear scale
mapped them to the same one; `anchor == 0` still yields the faint end;
`score == anchor` still yields hard red; overflow (`score > anchor`) clamps.

---

## Plan B — News page + favicons

### B1. Posts API (server)

New routes, independent of any configured dashboard widget instance:

- `GET /api/v1/posts/{source}` where `source` ∈ `hackernews` | `lobsters` →
  `PostsData` (the existing `{ last_24h, last_48h, last_week }` shape).
  Internally calls `widgets::posts::hacker_news` / `::lobsters` with a page
  default `NEWS_LIMIT = 50`. Server-cached with the same per-source TTL as the
  widgets (unify on the existing posts cache if practical; otherwise a parallel
  cache keyed by `("posts", source)`).

Unknown `source` → 404. Upstream failure serves stale payload if present, else
502, matching widget behavior.

### B2. Posts client + offline

`chaos-client` gains a typed call for the new route
(`posts_list(source) -> Result<PostsData, ClientError>`). The News page fetches
through it online; offline it falls back to the existing direct functions
(`chaos_client::posts::hacker_news`, `lobsters_direct`) via the same
`crate::offline::cached` / `cached_direct` pattern used by the dashboard widget.
Lobsters offline keeps its current constraint: it needs the Tauri HTTP shell.

### B3. `post_row_view` + favicons

Introduce `post_row_view(item: FeedItem, anchor: Option<u64>, source: Source)`
in `dashboard.rs`, **separate** from `feed_item_view` (which continues to serve
RSS `Feed` and `Releases` rows unchanged). Row layout:

- **Title** — taps navigate to the reader route `/news/{source}/{id}` (NOT the
  external link). `id` = HN `objectId` / lobsters `short_id`, carried on
  `FeedItem` (add `id: Option<String>` to `FeedItem`, populated by both the
  server and direct post parsers; `None` for RSS/releases).
- **Meta line** — log-colored score, comment count, relative age (as today).
- **Favicon (right)** — an `<a>` wrapping an `<img>` from the existing server
  icon proxy: `client.icon_url(&format!("fav:{host}"))` where `host` is the host
  of `item.url`. It links directly to `item.url` (the article), `target=_blank`.
  Posts with no `url` (Ask HN, jobs) use `fav:news.ycombinator.com` /
  `fav:lobste.rs` and the favicon links to the discussion (`comments_url`)
  instead. `<img>` `onerror` / CSS fallback shows a neutral glyph (offline or
  proxy miss).

`Source` is a small `Copy` enum (`HackerNews`, `Lobsters`) with
`as_str()`/`from_str` for the route segment and pref value.

### B4. `/news` page + navigation

New page `crates/chaos-ui/src/pages/news.rs`, route `/news` registered in
`lib.rs`. The page owns:

- `source: RwSignal<Source>` — persisted device pref `chaos-news-source`
  (default `HackerNews`), rendered as two sub-tabs at the top.
- `range: RwSignal<PostsTab>` — persisted device pref `chaos-news-range`
  (default `Day`), rendered as the 24h/48h/Week strip.
- A `LocalResource` keyed on `source` that loads `PostsData` (B2), then a `Memo`
  keyed on `range` selecting the window (same top-level-Memo discipline as A1),
  colored by one anchor over the union of the three windows.

Navigation:

- **Phone bottom bar** (`NAV_PRIMARY` in `lib.rs`): add `("/news", "▤", "News")`;
  bump the fixed array length. Order: Dash · Links · News · Weather · More
  (Search stays the trailing button).
- **Desktop topbar** (`lib.rs`): add a `<A href="/news">News</A>` alongside the
  existing links.
- **Phone dashboard** drops the HN/Lobsters widgets: the dashboard's widget
  render skips `Widget::HackerNews`/`Widget::Lobsters` when on a phone-width
  layout. Desktop keeps rendering them (via `PostsBody`, now using
  `post_row_view`, so desktop rows also get favicons + reader links). The
  phone/desktop distinction reuses the app's existing width mechanism (the same
  one that switches topbar↔tabbar); if that is CSS-only, gate the widget skip on
  a media-query-driven signal or render both and hide via CSS — pick the
  approach that matches the existing responsive pattern during implementation.

**Persist helpers:** add `news_source()/set_news_source()` and
`news_range()/set_news_range()` in `chaos-ui/src/lib.rs`, following the exact
`weather_combined` localStorage pattern.

---

## Plan C — Comment reader

### C1. Thread domain types (`chaos-domain`)

```rust
pub struct PostThread {
    pub id: String,
    pub title: String,
    pub url: Option<Url>,
    pub source: Option<String>,
    pub published: Option<DateTime<Utc>>,
    pub score: Option<u64>,
    pub comments: Option<u64>,          // total count
    pub comments_url: Option<Url>,
    pub body: Option<String>,           // sanitized self-text (Ask HN, story text)
    pub tree: Vec<Comment>,             // top-level comments, nested
}

pub struct Comment {
    pub author: Option<String>,
    pub html: String,                   // sanitized (server) or plain text (offline)
    pub published: Option<DateTime<Utc>>,
    pub children: Vec<Comment>,
}
```

All wasm-safe, no I/O, `PartialEq`/`Eq`/serde like the rest of the module.

### C2. Thread fetch + sanitization (server)

`GET /api/v1/posts/{source}/{id}/comments` → `PostThread`. Cached with a short
TTL (e.g. 5 min) keyed by `("thread", source, id)`; stale-on-failure.

- **HackerNews:** Algolia item API `https://hn.algolia.com/api/v1/items/{id}`
  returns the story root with a nested `children` tree; map recursively into
  `Comment`. `text` is HTML.
- **Lobsters:** `https://lobste.rs/s/{id}.json` returns story fields plus a flat
  `comments` array with a `depth` (and `short_id`/`parent`) — **rebuild the tree
  server-side** from depth/parent ordering. `comment` is HTML.
- **Sanitization:** comment/self-text HTML is untrusted. The server sanitizes to
  an allowlist via **`ammonia`**: tags `a,p,i,em,b,strong,code,pre,blockquote,br`
  (and `a[href]` with `rel=noreferrer`, forced `target=_blank`), everything else
  stripped. This is the only rendering path that emits HTML into the webview.

### C3. Thread client + offline

`chaos-client` gains `post_thread(source, id) -> Result<PostThread, ClientError>`.
Online it calls the server endpoint (already-sanitized HTML). Offline it falls
back to a **direct** fetch:

- HN direct via the Algolia item API (CORS-open; works in browser + shell).
- Lobsters direct via `/s/{id}.json` through the Tauri HTTP shell (no CORS;
  shell-only, same constraint as offline lobsters lists today).

The offline/wasm path **cannot run `ammonia`**, so it renders comment bodies as
**stripped plain text with linkified URLs** — a deliberate, safe degradation
that applies only when offline. A shared `chaos-domain` helper
`strip_to_text(html) -> String` does the stripping (regex/pull-based, wasm-safe)
and is used by the offline path; the online path keeps the server's sanitized
HTML verbatim.

### C4. Reader page

`/news/{source}/{id}` (registered in `lib.rs`, rendered by a `PostReader`
component in `pages/news.rs` or a sibling `pages/reader.rs`). Layout:

- **Header bar:** a back control (`<A href="/news">` / browser back), the story
  title (links to `url`), log-colored score, favicon, comment count.
- **Self-text** (if `body`) rendered under the header.
- **Comment tree:** nested `<ul>`; each node shows author · relative age · a
  collapse affordance, then the body. Online: body via Leptos `inner_html` from
  the server-sanitized `html`. Offline: body as escaped text.
- **Collapse gesture:** a short press on a comment toggles collapse of **its own
  subtree** — replies fold into a `[+N]` badge; press again to expand. Each
  comment folds independently. Depth is shown with an indent + a left color bar.
  Collapse state is per-node UI state (`RwSignal<bool>` per rendered node), not
  persisted.

Rendering `html` via `inner_html` is safe **only because** the server already
ran `ammonia`; the offline path never uses `inner_html` (escaped text only).

---

## Testing

- **A:** `score_color` monotonicity + low-end separation + clamp + `anchor==0`;
  `PostsBody` tab switch is covered by the existing option/list tests adapted to
  the component (the reactive fix is validated by the tab-switch test that
  currently can't be written against the nested closure).
- **B:** `Source` round-trip (`as_str`/`from_str`); favicon host derivation
  (host of `url`; fallback host when `url` is `None`); the pref helpers default
  correctly when unset; `post_row_view` links the title to the reader route and
  the favicon to the article. Server `/api/v1/posts/{source}` returns the three
  windows; unknown source → 404.
- **C:** HN item-tree mapping (nested children → `Comment` tree); lobsters
  depth-list → tree reconstruction (parent/child correctness at ≥3 levels);
  `ammonia` allowlist (script/onclick stripped, `a[href]` kept with
  `rel/target`); `strip_to_text` removes tags and linkifies bare URLs; endpoint
  404 on unknown source/id; offline fallback selection.

## Non-goals (YAGNI)

- No comment submission, voting, or auth against HN/lobsters — read-only.
- No comment pagination/lazy-loading — threads are fetched whole (HN/lobsters
  thread sizes are bounded enough for one payload).
- No collapse-state persistence across navigations.
- No rich HTML offline — plain text is the accepted offline degradation.
- No desktop removal of the widgets — desktop keeps its glance widgets.

## Security notes

- Comment HTML is the only untrusted content rendered into the webview. It is
  rendered via `inner_html` **only** after server-side `ammonia` sanitization.
  The offline path renders escaped text, never `inner_html`.
- New upstream egress: `hn.algolia.com/api/v1/items/*` (HN threads) and
  `lobste.rs/s/*.json` (lobsters threads) — deployment egress rules and the
  Tauri HTTP capability must allow these (in addition to the existing
  search/newest endpoints).
