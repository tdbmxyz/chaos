# Forward-Auth Identity + App Authentication + Greeting Design

**Date:** 2026-07-23
**Status:** Approved (design). Implementation split into two plans (A server, B client/UI).

## Problem

Chaos is deployed behind authentik forward-auth (traefik). Authentik authenticates
users and forwards `X-authentik-username` / `X-authentik-name` headers, but:

1. **Chaos ignores them** — it can't show who you are, so there's no way to
   confirm the SSO chain is reaching chaos.
2. **The Android/Tauri app can't get in** — every request 302-redirects to
   authentik's HTML login, which the app can't follow.

We want (1) a "Hello [name]" greeting proving you're logged in through SSO, and
(2) the app able to authenticate through authentik and reach the server as the
correct account.

## Decisions (from brainstorming, 2026-07-23)

- **Unify identity:** chaos trusts the forwarded `X-authentik-username` header as
  the identity (config-gated), so `/auth/me` returns that user and everything
  keys off the existing session signal. Direct/tailnet access keeps chaos's own
  cookie/Bearer login. This resolves the `Authorization` header collision (the
  app authenticates to authentik with Basic, not to chaos with Bearer) and
  matches ADR 0004's "external identity provider mints the identity" seam.
- **Auto-provision:** a forwarded username chaos hasn't seen creates a `users`
  row (`display_name` from `X-authentik-name`, empty `password_hash`).
- **Shared-secret guard:** chaos trusts the forwarded headers only when a
  `X-Chaos-Proxy-Secret` header matches a configured secret that only
  traefik/authentik sends — safe even if a direct route to chaos exists.
- **App auth = Basic:** the app stores an authentik username + app-password
  per device and sends `Authorization: Basic base64(user:token)`.

## Non-goals (YAGNI)

- No OIDC redirect flow (ADR 0004's other seam) — the header model is what the
  deployment already produces.
- No OS-keychain secret storage — the app-password lives in localStorage like
  today's session token.
- No forward-auth enabled by default — fully off unless the secret is set.
- No change to the auto-provisioned user's authorization (they're a normal
  chaos user; there are no roles/permissions in chaos today).

---

## Plan A — server

### A1. Config: `ForwardAuthConfig`

**Files:** `crates/chaos-server/src/config.rs`, `crates/chaos-server/chaos.example.toml`

Add a new optional section on `Config`, following the `NotificationsConfig`/
`HomeAssistantConfig` "feature off when its enabling field is None" pattern:

```rust
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(default)]
pub struct ForwardAuthConfig {
    /// Shared secret that the reverse proxy sends in `secret_header`. When
    /// `None`, forward-auth is DISABLED and no request header is ever trusted.
    pub secret: Option<String>,
    /// Header carrying the authenticated username (identity key).
    pub username_header: String,
    /// Header carrying the display name (used for auto-provisioned users).
    pub name_header: String,
    /// Header carrying the shared secret.
    pub secret_header: String,
}
impl Default for ForwardAuthConfig {
    fn default() -> Self {
        Self {
            secret: None,
            username_header: "x-authentik-username".into(),
            name_header: "x-authentik-name".into(),
            secret_header: "x-chaos-proxy-secret".into(),
        }
    }
}
```

Add `pub forward_auth: ForwardAuthConfig` to `Config` (with `#[serde(default)]`)
and to `Config::default`. `ForwardAuthConfig::enabled(&self) -> bool { self.secret.is_some() }`
helper. Header names are stored lowercase (axum `HeaderMap` lookups are
case-insensitive, but compare with `.get()` which is case-insensitive anyway —
store as given, look up via `HeaderName`). Document in `chaos.example.toml`:

```toml
# [forward_auth]
# Trust an authenticating reverse proxy (e.g. authentik via traefik). OFF unless
# `secret` is set. The proxy MUST send `secret_header` with this exact value on
# every request; a request without it is never trusted (so a direct/tailnet
# client cannot forge an identity). The proxy also forwards `username_header`
# (identity) and `name_header` (display name for auto-provisioned users).
# secret = "a-long-random-string"
# username_header = "X-authentik-username"
# name_header = "X-authentik-name"
# secret_header = "X-Chaos-Proxy-Secret"
```

### A2. DB: resolve-or-provision by username

**Files:** `crates/chaos-server/src/db_auth.rs`

```rust
impl Db {
    /// Resolve a user by username, creating one (empty password_hash → external
    /// identity only) if absent. `display_name` is used only on creation.
    pub async fn user_by_username_or_create(
        &self,
        username: &str,
        display_name: &str,
    ) -> Result<User> {
        if let Some(user) = self.user_by_username(username).await? {
            return Ok(user);
        }
        // create_user already exists (used by `add-user`); pass an empty hash.
        self.create_user(username, display_name, "").await
    }
}
```

Add `user_by_username(&self, username) -> Result<Option<User>>` if it doesn't
exist (a `SELECT … WHERE username = ?` mapping to `User`, mirroring
`user_with_password` minus the hash). Match the real `create_user` signature
(the map shows `create_user(username, display_name, password_hash) -> User`).

> Empty `password_hash` must make password login impossible: verify
> `verify_login` (auth.rs) rejects an empty stored hash (argon2 verify of any
> password against `""` fails / errors → false). If it could panic on an empty
> hash, guard `verify_login` to return false when the stored hash is empty. Add
> a test.

### A3. Extractor: forward-auth branch

**Files:** `crates/chaos-server/src/auth.rs`

Extend `AuthUser::from_request_parts`. Keep the token path first; add the
forward-auth fallback:

```rust
async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
    // 1. chaos session (Bearer/cookie) — unchanged, wins when present.
    if let Some(token) = request_token(&parts.headers)
        && let Ok(user) = state.db.user_by_session(&token_hash(&token)).await
    {
        return Ok(AuthUser(user));
    }
    // 2. trusted forward-auth header (only when configured + secret matches).
    if let Some(user) = forward_auth_user(&parts.headers, state).await? {
        return Ok(AuthUser(user));
    }
    Err(ApiError::Unauthorized)
}
```

```rust
/// Resolve the user from a trusted reverse-proxy header set, or `None` when
/// forward-auth is disabled / the secret doesn't match / no username present.
async fn forward_auth_user(headers: &HeaderMap, state: &AppState) -> Result<Option<User>, ApiError> {
    let cfg = &state.config.forward_auth;
    let Some(secret) = &cfg.secret else { return Ok(None) };           // disabled
    let sent = headers.get(cfg.secret_header.as_str()).and_then(|v| v.to_str().ok());
    if sent != Some(secret.as_str()) {                                 // wrong/absent secret
        return Ok(None);
    }
    let Some(username) = headers.get(cfg.username_header.as_str()).and_then(|v| v.to_str().ok())
    else { return Ok(None) };
    let username = username.trim();
    if username.is_empty() { return Ok(None); }
    let display = headers
        .get(cfg.name_header.as_str())
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(username);
    let user = state.db.user_by_username_or_create(username, display).await
        .map_err(|_| ApiError::Unauthorized)?;
    Ok(Some(user))
}
```

Note the current extractor swallows a token-lookup error into Unauthorized; the
new version must fall THROUGH to the forward-auth branch when the token path
finds nothing (e.g. no token at all), so structure it as "try token → try
forward-auth → Unauthorized". `optional_user_id` (used by search/attribution)
should ALSO honor forward-auth so those events attribute correctly — update it
to call `forward_auth_user` on the None path.

### A4. Deployment docs

**Files:** `docs/deployment.md`

Add a "Behind authentik (forward-auth)" section: set `forward_auth.secret`;
traefik middleware must (a) forward `X-authentik-username`/`X-authentik-name`
from the authentik outpost and (b) stamp `X-Chaos-Proxy-Secret: <secret>` on
requests to chaos (and strip any client-supplied copy). Note the app uses an
authentik app-password (Basic). Note the tailnet-direct route caveat: without
the secret header it falls back to chaos login, so a direct route is safe.

### A5. Tests (Plan A)

- `forward_auth_user`: disabled (no secret) → None; secret mismatch/absent →
  None; valid secret + new username → provisions + returns; valid secret +
  existing username → returns existing (no dup); missing username header → None;
  `name_header` absent → display_name falls back to username.
- Extractor precedence: a valid session token wins even if forward-auth headers
  are also present; no token + valid forward-auth → forward-auth user; no
  token + no/invalid forward-auth → 401.
- `user_by_username_or_create` idempotency (second call returns the same id).
- `verify_login` rejects an empty stored hash.

---

## Plan B — client & UI

### B1. Client Basic-auth

**Files:** `crates/chaos-client/src/lib.rs`

Add an optional credential to `ChaosClient` and a builder:

```rust
// field:
basic_auth: Option<(String, String)>,   // (username, app_password)
// builder (mirrors with_token):
pub fn with_basic_auth(mut self, creds: Option<(String, String)>) -> Self {
    self.basic_auth = creds;
    self
}
```

In `check_status`, Basic beats Bearer (behind authentik you use Basic; a chaos
Bearer would be pointless there):

```rust
let req = match (&self.basic_auth, &self.token) {
    (Some((u, p)), _) => req.basic_auth(u, Some(p)),
    (None, Some(token)) => req.bearer_auth(token),
    (None, None) => req,
};
```

> `reqwest::RequestBuilder::basic_auth` works on wasm + native. Keep `with_token`
> untouched.

### B2. Per-device authentik credential storage + helpers

**Files:** `crates/chaos-ui/src/lib.rs`

localStorage keys + helpers (mirror `stored_token`/`store_token`/`api_base_override`):

```rust
const AUTHENTIK_USER_KEY: &str = "chaos-authentik-user";
const AUTHENTIK_TOKEN_KEY: &str = "chaos-authentik-token";

pub(crate) fn authentik_creds() -> Option<(String, String)> {
    let u = pref(AUTHENTIK_USER_KEY)?;
    let t = pref(AUTHENTIK_TOKEN_KEY)?;
    (!u.is_empty() && !t.is_empty()).then_some((u, t))
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

Wire into `use_client()`: after `with_token`, apply `with_basic_auth(authentik_creds())`.
(When creds are set, Basic is used; else Bearer/cookie as today.)

### B3. Settings → Authentik section

**Files:** `crates/chaos-ui/src/pages/settings.rs`, `crates/chaos-web/styles.css` (only if needed)

A section mirroring the connect form: a username input, an app-password input
(`type="password"`), a **Save** button (`set_authentik_creds` then force a
re-probe / `me()` refresh so the greeting updates), and a **Forget** button
(`clear_authentik_creds`). Prefill username from `pref(AUTHENTIK_USER_KEY)`
(never render the stored token back). A one-line explainer: "For servers behind
authentik. Create an app password in authentik and enter it here."

### B4. "Hello [name]" greeting

**Files:** `crates/chaos-ui/src/lib.rs` (topbar), `crates/chaos-ui/src/pages/more.rs` (phone)

Replace the bare `{user.display_name}` in the topbar account area with
`"Hello " {user.display_name}`, and the `None` arm's bare "Sign in" with
"Hello stranger" alongside the existing sign-in affordance. Mirror in
`more.rs`. Driven entirely by the existing `session.0.get()` signal — no new
data flow (forward-auth makes `me()` return the identity).

```rust
{move || match session.0.get() {
    Some(user) => view! {
        <span class="topbar-user">"Hello " {user.display_name}</span>
        <button class="topbar-logout" ... >"Sign out"</button>
    }.into_any(),
    None => view! {
        <span class="topbar-user topbar-stranger">"Hello stranger"</span>
        <A href="/login">"Sign in"</A>
    }.into_any(),
}}
```

> Keep "Sign out" only meaningful for chaos-session logins; behind authentik the
> "logout" clears the local chaos token but the authentik session persists (the
> greeting stays because `me()` still resolves via the header). That's expected;
> a comment should note it.

### B5. Tests (Plan B)

- Pure: `authentik_creds` returns None when either key empty/absent, Some when
  both set; the client attach precedence (Basic over Bearer over none) — a small
  unit test if the client exposes the branch, else covered by a URL/header test
  if the crate has a request-building test harness.
- Greeting render: `Some(user)` → "Hello {name}"; `None` → "Hello stranger"
  (a `row_state`-style pure helper if extracted, else verified in-browser).

## Verification (before ship)

Headless-browser + a locally-run server with `forward_auth.secret` set:
- Simulate the proxy by hitting the API with `X-Chaos-Proxy-Secret` +
  `X-authentik-username`/`-name` and confirm `/auth/me` returns the (provisioned)
  user; without the secret → 401.
- In the app-auth path: set authentik creds in Settings, confirm the client
  sends `Authorization: Basic …` and the greeting shows "Hello {name}"; clear
  them → "Hello stranger".
- Direct chaos login (no forward-auth) still works and shows "Hello {name}".
