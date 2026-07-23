# Plan B — App Auth + Greeting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the app authenticate through authentik (Basic auth with a per-device app-password) and show a "Hello [name]" / "Hello stranger" greeting driven by the (now forward-auth-aware) session.

**Architecture:** `ChaosClient` gains an optional Basic-auth credential attached at its single auth point (Basic beats Bearer). Per-device authentik username + app-password live in localStorage with a Settings section to manage them. The topbar/More greeting reads the existing `session` signal.

**Tech Stack:** Leptos 0.8 CSR, reqwest (wasm), chaos-client. Depends on: Plan A merged. Spec: `docs/superpowers/specs/2026-07-23-forward-auth-and-greeting-design.md`.

**Verification (every task):** `cargo test -p chaos-ui` / `-p chaos-client`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`, `cargo check -p chaos-ui --target wasm32-unknown-unknown`.

Commit UNSIGNED, one per task, standard trailers. Do NOT push. Leave dirty schema files untouched.

---

### Task B1: Client Basic-auth

**Files:** `crates/chaos-client/src/lib.rs`

- [ ] **Step 1: Add the field + builder.** `ChaosClient` gains
`basic_auth: Option<(String, String)>` (default None in every constructor —
grep for where the struct is built and add `basic_auth: None`), and:

```rust
/// Attach HTTP Basic auth (for an authenticating reverse proxy such as
/// authentik). When set, it replaces the Bearer token on every request.
pub fn with_basic_auth(mut self, creds: Option<(String, String)>) -> Self {
    self.basic_auth = creds;
    self
}
```

- [ ] **Step 2: Attach in `check_status`.** Replace the existing
`match &self.token { Some(token) => req.bearer_auth(token), None => req }` with:

```rust
let req = match (&self.basic_auth, &self.token) {
    (Some((u, p)), _) => req.basic_auth(u, Some(p)),
    (None, Some(token)) => req.bearer_auth(token),
    (None, None) => req,
};
```

- [ ] **Step 3: Test** (mirror an existing client test that inspects a built
request, if the harness allows; otherwise a construction/round-trip test):

```rust
#[test]
fn basic_auth_builder_sets_creds() {
    let c = ChaosClient::new(/* real test constructor */).with_basic_auth(Some(("u".into(), "p".into())));
    assert!(c.has_basic_auth()); // add a #[cfg(test)] getter if the field is private
}
```

> If the crate already has request-building tests that can assert the
> `Authorization` header, prefer asserting `Authorization: Basic base64(u:p)` is
> present and Bearer is absent when both are set. Otherwise the getter test +
> the precedence logic suffice.

- [ ] **Step 4: Run tests + clippy.** Expected: green.

- [ ] **Step 5: Commit** `feat(client): optional Basic-auth (beats Bearer) for reverse-proxy auth`.

---

### Task B2: Per-device authentik credential storage

**Files:** `crates/chaos-ui/src/lib.rs`

- [ ] **Step 1: Write a failing test** for the pure accessor logic (the
localStorage read is browser-only, but the "both present" gate is pure — extract
it):

```rust
#[test]
fn authentik_creds_needs_both() {
    assert_eq!(authentik_creds_from(None, None), None);
    assert_eq!(authentik_creds_from(Some("u".into()), None), None);
    assert_eq!(authentik_creds_from(Some("u".into()), Some("".into())), None);
    assert_eq!(authentik_creds_from(Some("u".into()), Some("t".into())), Some(("u".into(), "t".into())));
}
```

- [ ] **Step 2: Run, verify fail.** `cargo test -p chaos-ui authentik_creds -v`. Expected: FAIL.

- [ ] **Step 3: Add keys + helpers** (mirror `stored_token`/`api_base_override`):

```rust
const AUTHENTIK_USER_KEY: &str = "chaos-authentik-user";
const AUTHENTIK_TOKEN_KEY: &str = "chaos-authentik-token";

fn authentik_creds_from(user: Option<String>, token: Option<String>) -> Option<(String, String)> {
    let (u, t) = (user?, token?);
    (!u.trim().is_empty() && !t.trim().is_empty()).then_some((u, t))
}
pub(crate) fn authentik_creds() -> Option<(String, String)> {
    authentik_creds_from(pref(AUTHENTIK_USER_KEY), pref(AUTHENTIK_TOKEN_KEY))
}
pub(crate) fn set_authentik_creds(user: &str, token: &str) {
    set_pref(AUTHENTIK_USER_KEY, user);
    set_pref(AUTHENTIK_TOKEN_KEY, token);
}
pub(crate) fn clear_authentik_creds() {
    set_pref(AUTHENTIK_USER_KEY, "");
    set_pref(AUTHENTIK_TOKEN_KEY, "");
}
```

- [ ] **Step 4: Wire into `use_client()`.** After the existing
`client.with_token(token)`, chain `.with_basic_auth(crate::authentik_creds())`.
(When creds are set, Basic is used; else Bearer/cookie as today.)

- [ ] **Step 5: Run tests + wasm check + clippy.** Expected: green.

- [ ] **Step 6: Commit** `feat(ui): per-device authentik credentials → client Basic-auth`.

---

### Task B3: Settings → Authentik section

**Files:** `crates/chaos-ui/src/pages/settings.rs`

- [ ] **Step 1: Add a section** below the existing connect/server form. A
username input (prefilled from `pref(AUTHENTIK_USER_KEY)`), an app-password
input (`type="password"`, NEVER prefilled from storage), **Save** and **Forget**
buttons, and a one-line explainer.

```rust
// sketch — match the file's existing signal/input idioms:
let ak_user = RwSignal::new(crate::pref(crate::AUTHENTIK_USER_KEY).unwrap_or_default());
let ak_token = RwSignal::new(String::new());
// Save: crate::set_authentik_creds(&ak_user.get_untracked(), &ak_token.get_untracked());
//       then trigger a re-probe / me() refresh so the greeting updates
//       (e.g. bump the connectivity/refresh the way the connect form does).
// Forget: crate::clear_authentik_creds(); ak_user.set(String::new()); ak_token.set(String::new());
view! {
    <section class="settings-authentik">
        <h3>"Authentik"</h3>
        <p class="muted">"For a server behind authentik. Create an app password in authentik and enter it here."</p>
        <input placeholder="authentik username" prop:value=move || ak_user.get()
               on:input=move |ev| ak_user.set(event_target_value(&ev)) />
        <input type="password" placeholder="app password" prop:value=move || ak_token.get()
               on:input=move |ev| ak_token.set(event_target_value(&ev)) />
        <button on:click=save>"Save"</button>
        <button on:click=forget>"Forget"</button>
    </section>
}
```

> After Save, force the session to refresh so "Hello stranger" flips to
> "Hello {name}" without a manual reload — use whatever the connect form uses to
> re-run the probe / `me()` (e.g. reload, or bump the connectivity signal). Match
> the existing pattern in `settings.rs`/`ServerGate`.

- [ ] **Step 2: Run wasm check + clippy + fmt.** Expected: green.

- [ ] **Step 3: Commit** `feat(settings): authentik login (username + app password)`.

---

### Task B4: "Hello [name]" greeting

**Files:** `crates/chaos-ui/src/lib.rs` (topbar), `crates/chaos-ui/src/pages/more.rs` (phone), `crates/chaos-web/styles.css` (optional)

- [ ] **Step 1: Topbar.** In the account render (lib.rs ~630-647), change the
`Some(user)` arm's `{user.display_name}` to `"Hello " {user.display_name}` and
the `None` arm to show `"Hello stranger"` alongside the sign-in link:

```rust
{move || match session.0.get() {
    Some(user) => view! {
        <span class="topbar-user">"Hello " {user.display_name}</span>
        <button class="topbar-logout" title="Sign out" on:click=move |ev| logout.run(ev)>"Sign out"</button>
    }.into_any(),
    None => view! {
        <span class="topbar-user topbar-stranger">"Hello stranger"</span>
        <A href="/login">"Sign in"</A>
    }.into_any(),
}}
```

- [ ] **Step 2: Phone (More page).** Mirror the same greeting in `more.rs`
(the account row): `"Hello " {user.display_name}` / `"Hello stranger"`.

- [ ] **Step 3: (optional) CSS** for `.topbar-stranger` (muted) if it should look
distinct — otherwise reuse `.topbar-user`.

- [ ] **Step 4: Note the logout caveat** in a code comment: behind authentik,
"Sign out" clears the local chaos token but the greeting persists because
`me()` still resolves via the forwarded header (the authentik session is
separate). That's expected.

- [ ] **Step 5: Run tests + wasm + clippy + fmt.** Expected: green.

- [ ] **Step 6: Commit** `feat(ui): 'Hello [name]' / 'Hello stranger' greeting`.

---

### Task B5: End-to-end verification + docs

- [ ] **Step 1: Headless-browser + local server verification** (per the session's
verify-in-browser practice). Run a server with `forward_auth.secret = "test-secret"`
(+ static dist). Verify:
  - **Proxy simulation (curl):** `GET /api/v1/auth/me` with headers
    `X-Chaos-Proxy-Secret: test-secret`, `X-authentik-username: so`,
    `X-authentik-name: So Balem` → 200 returning the (auto-provisioned) user;
    the SAME request WITHOUT the secret header → 401; wrong secret → 401.
  - **App-auth path (browser):** open Settings, enter authentik user + a token,
    Save; confirm the client now sends `Authorization: Basic …` (observe via a
    request, or that behind a simulated proxy `me()` resolves) and the topbar
    shows "Hello {name}"; Forget → "Hello stranger".
  - **Direct chaos login** still works and shows "Hello {name}".

- [ ] **Step 2: Full verification.** `cargo test --workspace`, clippy `-D warnings`,
fmt, wasm check, and release builds (`chaos-server`, `chaos-desktop`, trunk web).

- [ ] **Step 3: Docs.** Update `docs/HANDOFF.md` (forward-auth identity + app
Basic-auth + greeting) and `docs/ROADMAP.md`. Commit.

---

## Self-review notes
- Spec coverage: B1 client Basic-auth; B2 storage + use_client wiring; B3 settings; B4 greeting; B5 verify + docs. Covers §Plan B.
- Type consistency: `with_basic_auth(Option<(String,String)>)`, `authentik_creds()->Option<(String,String)>`, `authentik_creds_from(Option<String>,Option<String>)`, `set_/clear_authentik_creds`. Consistent.
- Depends on Plan A (server must trust the forwarded header for the greeting to show a name behind authentik).
- Security: app-password in localStorage like the session token; password input never prefilled from storage.
