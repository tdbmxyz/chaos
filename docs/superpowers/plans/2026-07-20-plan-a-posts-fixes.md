# Plan A — Posts Fixes (tab bug + log color scale) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the 24h/48h/Week tabs actually switch the posts list, and give scores a logarithmic heat scale so values stop clumping in yellow.

**Architecture:** The tab bug is a lost Leptos subscription: the tab-dependent list is built in a `move ||` closure nested inside the `.into_any()`-erased `match data.get()` branch, so `tab.set` notifies nothing. Fix by extracting the Posts body into a real `#[component]` that owns its `tab` signal and a top-level `Memo`. The color fix is a one-line change in `score_color` from linear to `ln(1+score)/ln(1+anchor)`.

**Tech Stack:** Leptos 0.8 CSR, `chaos-ui`. Spec: `docs/superpowers/specs/2026-07-20-news-tab-reader-design.md`.

**Verification commands (every task):**
- `cargo test -p chaos-ui`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`
- wasm check: `cargo check -p chaos-ui --target wasm32-unknown-unknown`

---

### Task A2 (color scale — do first; smaller, independent)

**Files:**
- Modify: `crates/chaos-ui/src/pages/dashboard.rs` (`score_color`, tests module)

- [ ] **Step 1: Write failing tests.** In `dashboard.rs`'s `mod tests`, add tests that pin the logarithmic behavior. The key property: two mid-range scores that the OLD linear scale collapsed into the same gradient bucket now land in different buckets.

```rust
#[test]
fn log_scale_separates_midrange_scores() {
    // Real clustered set: anchor is the top score.
    let anchor = 2146u64;
    // Under linear score/anchor, 334 and 497 both sit ~0.15-0.23 → both in the
    // first gradient segment (faint→yellow). Under log they diverge.
    let c334 = score_color(334, anchor);
    let c497 = score_color(497, anchor);
    assert_ne!(c334, c497, "mid-range scores must be distinguishable");
}

#[test]
fn log_scale_is_monotonic() {
    let anchor = 2146u64;
    // Higher score → not-lighter color: compare the red channel (first two hex
    // chars), which increases toward hard red.
    let red = |s: &str| u8::from_str_radix(&s[1..3], 16).unwrap();
    assert!(red(&score_color(865, anchor)) >= red(&score_color(193, anchor)));
    assert!(red(&score_color(2146, anchor)) >= red(&score_color(865, anchor)));
}

#[test]
fn log_scale_anchor_hits_hard_red() {
    assert_eq!(score_color(2146, 2146), "#ff2200");
}

#[test]
fn log_scale_overflow_clamps_to_hard_red() {
    assert_eq!(score_color(9000, 2146), "#ff2200");
}

#[test]
fn log_scale_zero_anchor_is_faint() {
    assert_eq!(score_color(0, 0), "#e8d288");
}
```

- [ ] **Step 2: Run tests, verify `log_scale_separates_midrange_scores` fails** under the current linear scale. Run: `cargo test -p chaos-ui score_color 2>&1 | tail; cargo test -p chaos-ui log_scale 2>&1 | tail`. Expected: the separation/monotonic tests fail (linear collapses them or the exact hex differs).

- [ ] **Step 3: Change `score_color` to logarithmic.** Replace only the `t` computation:

```rust
fn score_color(score: u64, anchor: u64) -> String {
    let t = if anchor == 0 {
        0.0
    } else {
        (((score as f64) + 1.0).ln() / ((anchor as f64) + 1.0).ln()).clamp(0.0, 1.0)
    };
    let pos = t * (HEAT_STOPS.len() - 1) as f64;
    let i = (pos as usize).min(HEAT_STOPS.len() - 2);
    let f = pos - i as f64;
    let (lo, hi) = (HEAT_STOPS[i], HEAT_STOPS[i + 1]);
    let lerp = |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * f).round() as u8;
    format!("#{:02x}{:02x}{:02x}", lerp(lo.0, hi.0), lerp(lo.1, hi.1), lerp(lo.2, hi.2))
}
```

Leave `score_anchor` (p99-over-union) and `HEAT_STOPS` unchanged.

- [ ] **Step 4: Run all verification commands.** Expected: green.

- [ ] **Step 5: Commit** (unsigned):

```bash
git add crates/chaos-ui/src/pages/dashboard.rs
git -c commit.gpgsign=false commit -m "fix(posts): logarithmic score heat scale so mid-range scores separate"
```

---

### Task A1 (tab-subscription bug)

**Files:**
- Modify: `crates/chaos-ui/src/pages/dashboard.rs` (extract `PostsBody` component; Posts match arm)

**Context:** Current Posts arm (~dashboard.rs:297-347) computes `anchor`, an
`items(t)` closure, renders the `.posts-tabs` buttons, then renders the list in a
`move || { let items = items(tab.get()); … }` closure nested inside the outer
`{move || match data.get()}` block. `tab`/`collapsed` are created at ~269-270.
`PostsTab` enum is at ~395. `Collapsible` is at ~486. `feed_item_view` at ~871.

- [ ] **Step 1: Write a failing test for tab switching.** Because the current
structure can't be reactively tested in isolation, the test targets the extracted
component's selection logic. Add a pure helper `posts_window(posts, tab)` and test
it, then have `PostsBody` use it:

```rust
#[test]
fn posts_window_selects_by_tab() {
    let mk = |title: &str| FeedItem {
        title: title.into(), url: None, source: None, published: None,
        score: None, comments: None, comments_url: None,
    };
    let posts = PostsData {
        last_24h: vec![mk("a")],
        last_48h: vec![mk("a"), mk("b")],
        last_week: vec![mk("a"), mk("b"), mk("c")],
    };
    assert_eq!(posts_window(&posts, PostsTab::Day).len(), 1);
    assert_eq!(posts_window(&posts, PostsTab::TwoDays).len(), 2);
    assert_eq!(posts_window(&posts, PostsTab::Week).len(), 3);
}
```

> If `FeedItem` has additional fields at implementation time, fill them from the
> struct definition in `chaos-domain/src/dashboard.rs`; keep the test's intent.

- [ ] **Step 2: Run test, verify it fails** (`posts_window` not defined). Run: `cargo test -p chaos-ui posts_window -v`. Expected: FAIL, unresolved `posts_window`.

- [ ] **Step 3: Add the pure helper.**

```rust
fn posts_window(posts: &PostsData, tab: PostsTab) -> Vec<FeedItem> {
    match tab {
        PostsTab::Day => posts.last_24h.clone(),
        PostsTab::TwoDays => posts.last_48h.clone(),
        PostsTab::Week => posts.last_week.clone(),
    }
}
```

- [ ] **Step 4: Run test, verify it passes.** Run: `cargo test -p chaos-ui posts_window -v`. Expected: PASS.

- [ ] **Step 5: Extract the `PostsBody` component.** Add a component that owns
`tab` + a top-level `Memo`, so the subscription lives in a real owner scope:

```rust
#[component]
fn PostsBody(posts: PostsData, anchor: Option<u64>) -> impl IntoView {
    let collapsed = RwSignal::new(true);
    let tab = RwSignal::new(PostsTab::Day);
    let shown = Memo::new(move |_| posts_window(&posts, tab.get()));
    view! {
        <div class="posts-tabs">
            {[(PostsTab::Day, "24h"), (PostsTab::TwoDays, "48h"), (PostsTab::Week, "Week")]
                .map(|(t, label)| view! {
                    <button class:active=move || tab.get() == t on:click=move |_| tab.set(t)>
                        {label}
                    </button>
                })}
        </div>
        {move || {
            let items = shown.get();
            let count = items.len();
            view! {
                <Collapsible count collapsed>
                    <ul class="feed-list">
                        {items.into_iter().map(|item| feed_item_view(item, anchor)).collect_view()}
                    </ul>
                </Collapsible>
            }
        }}
    }
}
```

- [ ] **Step 6: Replace the Posts match arm** to compute the anchor and delegate:

```rust
Some(Ok((WidgetData::Posts(posts), _))) => {
    let anchor = score_anchor(
        posts.last_24h.iter().chain(&posts.last_48h).chain(&posts.last_week).map(|i| i.score),
    );
    view! { <PostsBody posts anchor/> }.into_any()
}
```

Delete the now-unused `items`/`tab`/`collapsed` locals from the old arm (they moved
into `PostsBody`). Keep `PostsTab`, `score_anchor`, `feed_item_view` as-is.

- [ ] **Step 7: Run all verification commands.** Expected: green.

- [ ] **Step 8: Manual sanity note** (for the executor to record, not a blocker):
the reactive fix cannot be unit-tested end-to-end without a DOM; the `posts_window`
test plus the top-level-`Memo` structure are the guardrails. State this in the
commit body.

- [ ] **Step 9: Commit** (unsigned):

```bash
git add crates/chaos-ui/src/pages/dashboard.rs
git -c commit.gpgsign=false commit -m "fix(posts): tab clicks switch the list (top-level Memo in PostsBody)

The tab-dependent list lived in a closure nested inside the type-erased
match-arm, dropping its subscription to the tab signal. Extracting a
PostsBody component with a top-level Memo restores reactivity. Verified
via posts_window unit test; the reactive wiring is structural."
```

---

## Self-review notes
- Spec coverage: A1 covers §Plan A / A1 (tab bug); A2 covers §Plan A / A2 (log
  scale). Both spec requirements have tasks.
- Type consistency: `posts_window(&PostsData, PostsTab) -> Vec<FeedItem>`,
  `score_color(u64, u64) -> String`, `score_anchor(...) -> Option<u64>`,
  `PostsBody(posts: PostsData, anchor: Option<u64>)` — consistent across tasks.
- Order: A2 before A1 (independent, smaller). Both land in the same file; the
  executor should do them sequentially, not in parallel, to avoid conflicts.
