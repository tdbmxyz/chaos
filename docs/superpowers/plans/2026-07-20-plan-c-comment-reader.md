# Plan C — Comment Reader Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Tapping a post opens `/news/{source}/{id}` — the story header plus a collapsible comment tree, fetched from a server endpoint (sanitized HTML) with an offline direct fallback (plain text).

**Architecture:** `chaos-domain` gains `PostThread`/`Comment` plus a wasm-safe `strip_to_text`. The server adds `GET /api/v1/posts/{source}/{id}/comments`: HN via Algolia's item API (nested tree), lobsters via `/s/{id}.json` (flat depth-list → rebuilt tree), then `ammonia`-sanitized HTML. The client's `post_thread(source, id)` calls the endpoint online and direct-fetches offline (plain text, no `ammonia` in wasm). A `PostReader` component renders the tree with per-node collapse.

**Tech Stack:** Leptos 0.8 CSR, Axum, `ammonia` (server), `chaos-domain`/`chaos-client`. Depends on: Plan B merged (`Source`, `FeedItem.id`, `/news` page, `post_row_view` links). Spec: `docs/superpowers/specs/2026-07-20-news-tab-reader-design.md`.

**Verification commands (every task):**
- `cargo test -p <crate touched>`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`
- wasm check (UI/domain): `cargo check -p chaos-ui --target wasm32-unknown-unknown`

---

### Task C1: `PostThread`/`Comment` + `strip_to_text` (domain)

**Files:**
- Modify: `crates/chaos-domain/src/dashboard.rs` (types)
- Create: `crates/chaos-domain/src/sanitize.rs` (`strip_to_text`) + wire in `lib.rs`
- Test: both files' `mod tests`

- [ ] **Step 1: Write failing tests for `strip_to_text`.**

```rust
#[test]
fn strip_removes_tags_keeps_text() {
    assert_eq!(strip_to_text("<p>hello <b>world</b></p>"), "hello world");
}
#[test]
fn strip_decodes_basic_entities() {
    assert_eq!(strip_to_text("a &amp; b &lt;c&gt;"), "a & b <c>");
}
#[test]
fn strip_linkifies_bare_urls_as_plain_text() {
    // URLs survive as readable text (not turned into HTML).
    assert_eq!(strip_to_text("see https://x.io/y here"), "see https://x.io/y here");
}
#[test]
fn strip_collapses_paragraph_breaks_to_newlines() {
    assert_eq!(strip_to_text("<p>a</p><p>b</p>"), "a\nb");
}
```

- [ ] **Step 2: Run, verify fail.** Run: `cargo test -p chaos-domain strip_ -v`. Expected: FAIL.

- [ ] **Step 3: Implement `strip_to_text`** (regex-free, wasm-safe char scan; no
external HTML parser so it builds on `wasm32`):

```rust
/// Best-effort conversion of provider comment HTML to plain text: drop tags,
/// map <p>/<br> to newlines, decode the five predefined XML entities. Used only
/// on the offline path, which must never emit HTML into the webview.
pub fn strip_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut chars = html.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '<' => {
                // read the tag name to decide on a newline for <p>/<br>
                let mut tag = String::new();
                for t in chars.by_ref() {
                    if t == '>' { break; }
                    tag.push(t);
                }
                let name = tag.trim_start_matches('/').trim().to_ascii_lowercase();
                if name.starts_with('p') || name.starts_with("br") {
                    if !out.ends_with('\n') && !out.is_empty() { out.push('\n'); }
                }
            }
            '&' => {
                let mut ent = String::new();
                while let Some(&n) = chars.peek() {
                    if n == ';' { chars.next(); break; }
                    if ent.len() > 6 { break; }
                    ent.push(n); chars.next();
                }
                out.push_str(match ent.as_str() {
                    "amp" => "&", "lt" => "<", "gt" => ">",
                    "quot" => "\"", "#39" | "apos" => "'",
                    _ => "", // unknown entity dropped
                });
            }
            _ => out.push(c),
        }
    }
    out.trim().to_string()
}
```

> Add `pub mod sanitize;` in `chaos-domain/src/lib.rs` and re-export
> `pub use sanitize::strip_to_text;`. Refine the entity/tag handling until the four
> tests pass; keep it dependency-free.

- [ ] **Step 4: Add `PostThread`/`Comment` types** (in `dashboard.rs`, same derives
as siblings — `Clone, Debug, PartialEq, Eq, Serialize, Deserialize`):

```rust
pub struct PostThread {
    pub id: String,
    pub title: String,
    pub url: Option<Url>,
    pub source: Option<String>,
    pub published: Option<DateTime<Utc>>,
    pub score: Option<u64>,
    pub comments: Option<u64>,
    pub comments_url: Option<Url>,
    pub body: Option<String>,     // sanitized self-text (Ask HN / story text)
    pub tree: Vec<Comment>,
}

pub struct Comment {
    pub author: Option<String>,
    pub html: String,             // sanitized (server) or plain text (offline)
    pub published: Option<DateTime<Utc>>,
    pub children: Vec<Comment>,
}
```

- [ ] **Step 5: Add a tree-shape test** (guards the type + serde round-trip):

```rust
#[test]
fn comment_tree_round_trips() {
    let t = PostThread { id: "1".into(), title: "t".into(), url: None, source: None,
        published: None, score: Some(3), comments: Some(1), comments_url: None,
        body: None, tree: vec![Comment { author: Some("a".into()), html: "hi".into(),
            published: None, children: vec![Comment { author: None, html: "re".into(),
                published: None, children: vec![] }] }] };
    let s = serde_json::to_string(&t).unwrap();
    assert_eq!(serde_json::from_str::<PostThread>(&s).unwrap(), t);
}
```

- [ ] **Step 6: Run tests + wasm check** (`cargo check -p chaos-domain --target wasm32-unknown-unknown`). Expected: green.

- [ ] **Step 7: Commit.**

```bash
git add crates/chaos-domain
git -c commit.gpgsign=false commit -m "feat(domain): PostThread/Comment types + wasm-safe strip_to_text"
```

---

### Task C2: HN + lobsters thread fetch + sanitize (server)

**Files:**
- Modify: `crates/chaos-server/Cargo.toml` (add `ammonia`)
- Create: `crates/chaos-server/src/widgets/threads.rs` (fetch + map + sanitize)
- Modify: `crates/chaos-server/src/widgets/mod.rs` (`mod threads;` + `post_thread` method + cache)

- [ ] **Step 1: Add `ammonia`.** In `crates/chaos-server/Cargo.toml` dependencies:
`ammonia = "4"` (verify the current 4.x version resolves; run `cargo update -p ammonia` after adding). Commit this as part of Step 8.

- [ ] **Step 2: Write failing tests** in `threads.rs` for the pure mappers +
sanitizer. Use small fixture JSON strings.

```rust
#[test]
fn hn_item_maps_nested_tree() {
    let json = r#"{"id":1,"title":"Story","points":10,"url":"https://x.io",
      "author":"u","created_at_i":1700000000,
      "children":[{"id":2,"author":"a","text":"<p>top</p>","created_at_i":1700000100,
        "children":[{"id":3,"author":"b","text":"reply","created_at_i":1700000200,"children":[]}]}]}"#;
    let t = map_hn_item(json).unwrap();
    assert_eq!(t.title, "Story");
    assert_eq!(t.tree.len(), 1);
    assert_eq!(t.tree[0].children.len(), 1);
    assert_eq!(t.tree[0].children[0].author.as_deref(), Some("b"));
}

#[test]
fn lobsters_depth_list_rebuilds_tree() {
    // lobste.rs /s/{id}.json: flat comments with `depth` (1-based) in pre-order.
    let json = r#"{"short_id":"abc","title":"S","score":4,"url":"https://x.io",
      "comments":[
        {"short_id":"c1","comment":"a","depth":1,"commenting_user":"u1","created_at":"2024-01-01T00:00:00Z"},
        {"short_id":"c2","comment":"b","depth":2,"commenting_user":"u2","created_at":"2024-01-01T00:01:00Z"},
        {"short_id":"c3","comment":"c","depth":1,"commenting_user":"u3","created_at":"2024-01-01T00:02:00Z"}]}"#;
    let t = map_lobsters_story(json).unwrap();
    assert_eq!(t.tree.len(), 2);          // c1, c3 at top level
    assert_eq!(t.tree[0].children.len(), 1); // c2 under c1
}

#[test]
fn sanitize_strips_script_keeps_links() {
    let dirty = r#"<p onclick="x">hi</p><script>evil()</script><a href="https://x.io">l</a>"#;
    let clean = sanitize_html(dirty);
    assert!(!clean.contains("script"));
    assert!(!clean.contains("onclick"));
    assert!(clean.contains("href=\"https://x.io\""));
    assert!(clean.contains("rel=") && clean.contains("noreferrer"));
}
```

- [ ] **Step 3: Run, verify fail.** Run: `cargo test -p chaos-server hn_item_maps map_lobsters sanitize_strips -v`. Expected: FAIL.

- [ ] **Step 4: Implement `sanitize_html`** with an `ammonia::Builder` allowlist:

```rust
use std::collections::HashSet;

pub(crate) fn sanitize_html(dirty: &str) -> String {
    ammonia::Builder::new()
        .tags(HashSet::from(["a","p","i","em","b","strong","code","pre","blockquote","br"]))
        .link_rel(Some("noreferrer noopener"))
        .add_tag_attributes("a", &["href"])
        .url_schemes(HashSet::from(["http","https"]))
        .clean(dirty)
        .to_string()
}
```

> `ammonia` forces `target=_blank` semantics via `link_rel` + its default anchor
> handling; verify the test's `rel`/`href` assertions against the exact 4.x output
> and adjust the assertions (not the allowlist) if the attribute ordering differs.

- [ ] **Step 5: Implement the mappers** (`map_hn_item`, `map_lobsters_story`),
sanitizing every `html`/`body` through `sanitize_html`:
  - `map_hn_item`: deserialize the Algolia item (recursive `children`), recurse into
    `Comment`, `sanitize_html(text)`; root becomes `PostThread` (score=`points`,
    `comments_url` = `https://news.ycombinator.com/item?id={id}`, `body` = sanitized
    root `text` when the story is a text post).
  - `map_lobsters_story`: deserialize story + flat `comments`; rebuild the tree by
    walking the pre-ordered list with a depth stack (each item attaches under the
    last item at `depth-1`); `sanitize_html(comment)`; `comments_url` = story
    `comments_url`/`short_id_url` when present.

- [ ] **Step 6: Add the async fetchers + cached `post_thread`** in `widgets/mod.rs`:

```rust
pub async fn post_thread(&self, source: chaos_domain::Source, id: &str)
    -> Result<PostThread, WidgetError>
{
    use chaos_domain::Source;
    let key = format!("thread:{}:{id}", source.as_str());
    let ttl = std::time::Duration::from_secs(300);
    let http = self.http.clone();
    let id = id.to_string();
    let fut = async move {
        match source {
            Source::HackerNews => threads::fetch_hn(&http, &id).await,
            Source::Lobsters => threads::fetch_lobsters(&http, &id).await,
        }
    };
    self.cached_fetch(key, ttl, fut).await
}
```

`threads::fetch_hn` GETs `https://hn.algolia.com/api/v1/items/{id}` → `map_hn_item`;
`threads::fetch_lobsters` GETs `https://lobste.rs/s/{id}.json` → `map_lobsters_story`.
Match the real `cached_fetch`/`WidgetError` types (the cache stores `PostThread`; if
`StaleCache` is typed to `WidgetData`, add a second `StaleCache<String, PostThread>`
field to `WidgetService` or a small generic cache — pick the smaller change).

- [ ] **Step 7: Run `cargo test -p chaos-server` + clippy + fmt.** Expected: green.

- [ ] **Step 8: Commit.**

```bash
git add crates/chaos-server/Cargo.toml Cargo.lock crates/chaos-server/src/widgets
git -c commit.gpgsign=false commit -m "feat(server): comment-thread fetch (HN item API, lobsters story) + ammonia sanitize"
```

---

### Task C3: `/api/v1/posts/{source}/{id}/comments` route (server)

**Files:**
- Modify: `crates/chaos-server/src/api/widgets.rs` (or `api/posts.rs`) (handler)
- Modify: `crates/chaos-server/src/api/mod.rs` (route)

- [ ] **Step 1: Add the handler.**

```rust
pub async fn post_thread(
    State(state): State<AppState>,
    Path((source, id)): Path<(String, String)>,
) -> Result<Json<PostThread>, ApiError> {
    let source = chaos_domain::Source::from_str(&source).ok_or(ApiError::NotFound)?;
    let thread = state.widgets.post_thread(source, &id).await.map_err(ApiError::from)?;
    Ok(Json(thread))
}
```

- [ ] **Step 2: Add the route** in `api/mod.rs` next to `/posts/{source}`:

```rust
.route("/posts/{source}/{id}/comments", get(widgets::post_thread))
```

- [ ] **Step 3: Test 404 on unknown source** (mirror B1's endpoint test):

```rust
#[tokio::test]
async fn thread_unknown_source_is_404() {
    // GET /api/v1/posts/nope/1/comments -> 404
}
```

- [ ] **Step 4: Run `cargo test -p chaos-server` + clippy.** Expected: green.

- [ ] **Step 5: Commit.**

```bash
git add crates/chaos-server/src/api
git -c commit.gpgsign=false commit -m "feat(server): GET /api/v1/posts/{source}/{id}/comments"
```

---

### Task C4: `post_thread` client + offline direct fallback

**Files:**
- Modify: `crates/chaos-client/src/lib.rs` (`post_thread` typed call)
- Modify: `crates/chaos-client/src/posts.rs` (direct thread fetch + `strip_to_text` mapping)

- [ ] **Step 1: Write failing tests** for the direct mappers (client side, plain
text). Reuse the C2 fixture JSON; assert the tree shape and that comment `html` is
plain text (no `<`):

```rust
#[test]
fn direct_hn_thread_is_plain_text() {
    let json = /* same HN fixture as C2 */;
    let t = parse_hn_thread(json).unwrap();
    assert_eq!(t.tree[0].html, "top");          // <p> stripped
    assert!(!t.tree[0].html.contains('<'));
}
```

- [ ] **Step 2: Run, verify fail.** Run: `cargo test -p chaos-client thread -v`. Expected: FAIL.

- [ ] **Step 3: Implement direct parsers** `parse_hn_thread`/`parse_lobsters_thread`
in `chaos-client/src/posts.rs`, identical tree-building to C2 but mapping bodies via
`chaos_domain::strip_to_text` instead of `ammonia`. Add
`hacker_news_thread(http, id)` / `lobsters_thread_url(id)` helpers mirroring the
existing `hacker_news`/`lobsters_page_url` shape.

- [ ] **Step 4: Add the typed server call** in `chaos-client/src/lib.rs`:

```rust
pub async fn post_thread(&self, source: chaos_domain::Source, id: &str) -> Result<PostThread> {
    self.get(&format!("api/v1/posts/{}/{}/comments", source.as_str(), id)).await
}
```

- [ ] **Step 5: Run `cargo test -p chaos-client` + clippy.** Expected: green.

- [ ] **Step 6: Commit.**

```bash
git add crates/chaos-client
git -c commit.gpgsign=false commit -m "feat(client): post_thread server call + offline direct (plain-text) fallback"
```

---

### Task C5: `PostReader` page + route + collapsible tree

**Files:**
- Create: `crates/chaos-ui/src/pages/reader.rs` (`PostReader`)
- Modify: `crates/chaos-ui/src/pages/mod.rs` (export)
- Modify: `crates/chaos-ui/src/lib.rs` (route `/news/{source}/{id}`; remove B5 stub if present)
- Modify: `crates/chaos-web/styles.css` (reader + comment tree)

- [ ] **Step 1: Write a failing test for the offline/online body-render decision**
(pure helper so it's unit-testable):

```rust
#[test]
fn reader_renders_html_online_text_offline() {
    // online → inner_html path; offline → escaped text path.
    assert!(use_inner_html(crate::offline::Connectivity::Online));
    assert!(!use_inner_html(crate::offline::Connectivity::Offline));
}
```

- [ ] **Step 2: Run, verify fail.** Run: `cargo test -p chaos-ui use_inner_html -v`. Expected: FAIL.

- [ ] **Step 3: Add `use_inner_html`** (the guardrail that `inner_html` is used
ONLY for server-sanitized content):

```rust
/// `inner_html` is safe only for server-sanitized bodies (online). Offline
/// bodies are plain text and must render as escaped text.
fn use_inner_html(conn: crate::offline::Connectivity) -> bool {
    conn == crate::offline::Connectivity::Online
}
```

- [ ] **Step 4: Build `PostReader`.** Route params `source`/`id` (via
`leptos_router` params). Loads the thread: online `client.post_thread(source, id)`,
offline the direct parser via the Tauri/`weather_fetch::http` path (mirror
`DirectFeed`). Renders header + tree.

```rust
#[component]
pub fn PostReader() -> impl IntoView {
    let client = expect_context::<Client>();
    let conn = crate::offline::use_connectivity();
    let params = use_params_map();
    let source = move || params.read().get("source")
        .and_then(|s| chaos_domain::Source::from_str(&s));
    let id = move || params.read().get("id").unwrap_or_default();

    let thread = LocalResource::new(move || {
        let (client, id) = (client.clone(), id());
        let src = source();
        async move {
            match src {
                Some(s) => load_thread(s, &id, conn, &client).await, // Result<PostThread,String>
                None => Err("unknown source".to_string()),
            }
        }
    });

    view! {
        <section class="reader">
            <a class="reader-back" href="/news">"‹ News"</a>
            // Suspend over `thread`: header (title→url, log-colored score, favicon),
            // optional self-text `body`, then <ComentTree nodes=thread.tree/>.
        </section>
    }
}
```

- [ ] **Step 5: Build the recursive comment view with per-node collapse.**

```rust
fn comment_view(node: Comment, online: bool) -> impl IntoView {
    let collapsed = RwSignal::new(false);
    let child_count = count_descendants(&node); // for the [+N] badge
    let header = format!("{} · {}",
        node.author.as_deref().unwrap_or("[deleted]"),
        node.published.map(rel_time).unwrap_or_default());
    let body = node.html.clone();
    let children = node.children;
    view! {
        <li class="comment">
            <div class="comment-head" on:click=move |_| collapsed.update(|c| *c = !*c)>
                <span class="comment-meta">{header}</span>
                {move || collapsed.get().then(|| view! { <span class="comment-badge">{format!("[+{child_count}]")}</span> })}
            </div>
            <Show when=move || !collapsed.get()>
                {
                    // body: inner_html only when online (server-sanitized)
                    if online {
                        view! { <div class="comment-body" inner_html=body.clone()></div> }.into_any()
                    } else {
                        view! { <div class="comment-body">{body.clone()}</div> }.into_any()
                    }
                }
                <ul class="comment-children">
                    {children.clone().into_iter().map(|c| comment_view(c, online)).collect_view()}
                </ul>
            </Show>
        </li>
    }
}

fn count_descendants(node: &Comment) -> usize {
    node.children.len() + node.children.iter().map(count_descendants).sum::<usize>()
}
```

> `online` is computed once from `use_inner_html(conn.get())` at render time and
> threaded down (all nodes share one connectivity verdict). Add a `count_descendants`
> unit test (a 3-level fixture → correct total).

- [ ] **Step 6: Register route + page.** `pages/mod.rs`: `mod reader; pub use reader::PostReader;`.
`lib.rs` `<Routes>`: `<Route path=path!("/news/{source}/{id}") view=pages::PostReader/>`
(remove the B5 "coming soon" stub route if it was added). Confirm the `/news`
static route and the `/news/{source}/{id}` param route don't conflict (static wins
in `leptos_router`; if not, order the param route after).

- [ ] **Step 7: Style the reader + tree.** In `styles.css`:

```css
.reader-back { display: inline-block; margin-bottom: .5rem; color: var(--accent); }
.comment { list-style: none; }
.comment-children { list-style: none; margin: 0 0 0 .8rem; padding-left: .6rem;
    border-left: 2px solid var(--border); }
.comment-head { cursor: pointer; user-select: none; }
.comment-meta { font-size: .8rem; color: var(--muted); }
.comment-badge { margin-left: .4rem; font-size: .75rem; color: var(--accent); }
.comment-body { margin: .2rem 0 .5rem; }
.comment-body pre { overflow-x: auto; }
```

- [ ] **Step 8: Add the egress/capability updates.** Add
`https://hn.algolia.com/api/v1/items/*` and `https://lobste.rs/s/*` to the Tauri HTTP
capability (`crates/chaos-desktop/capabilities/default.json`) and regenerate
`gen/schemas/capabilities.json`. Note in the commit that deployment egress must
allow the two new upstream paths.

- [ ] **Step 9: Run all verification commands + trunk build + release builds.** Run:
`cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check && cargo check -p chaos-ui --target wasm32-unknown-unknown && cargo build --release -p chaos-server && cargo build --release -p chaos-desktop`.
Expected: green.

- [ ] **Step 10: Commit.**

```bash
git add crates/chaos-ui crates/chaos-web/styles.css crates/chaos-desktop
git -c commit.gpgsign=false commit -m "feat(news): in-app comment reader with collapsible tree (/news/{source}/{id})"
```

---

## Self-review notes
- Spec coverage: C1 = types + `strip_to_text`; C2 = fetch + `ammonia`; C3 = route;
  C4 = client + offline plain-text; C5 = reader page, collapse gesture, `inner_html`
  only-when-online guardrail, egress/capability. All §Plan C items covered.
- Security: `inner_html` is gated behind `use_inner_html(Online)` and only ever
  receives server-`ammonia`-sanitized HTML; offline uses escaped text. Stated in C5.
- Type consistency: `PostThread`/`Comment` (domain), `sanitize_html(&str)->String`
  (server), `strip_to_text(&str)->String` (domain), `post_thread(Source,&str)`
  (client + server), `comment_view(Comment, bool)`, `use_inner_html(Connectivity)`.
  Consistent across tasks.
- Collapse gesture matches the spec: short press toggles a node's own subtree; each
  node folds independently; `[+N]` badge shows descendant count.
- Depends on Plan B's `Source`, `FeedItem.id`, `/news` page, and reader links.
