# Plan A — Forward-Auth Server Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let chaos trust an authenticating reverse proxy: resolve (auto-provisioning) the user from a forwarded `X-authentik-username` header, gated by a shared-secret header, config-off by default.

**Architecture:** A new `ForwardAuthConfig` (feature off unless `secret` is set). The `AuthUser` extractor tries the existing session token first, then a forward-auth fallback that trusts the username header only when the secret header matches. A DB resolve-or-provision method creates users on first contact with an empty password hash.

**Tech Stack:** Axum, sqlx/SQLite, figment config. Spec: `docs/superpowers/specs/2026-07-23-forward-auth-and-greeting-design.md`.

**Verification (every task):** `cargo test -p chaos-server`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`.

Commit UNSIGNED (`git -c commit.gpgsign=false`), one per task, trailers:
`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
`Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP`
Do NOT push. Leave the dirty `android-schema.json`/`mobile-schema.json` files untouched.

---

### Task A1: `ForwardAuthConfig`

**Files:** `crates/chaos-server/src/config.rs`, `crates/chaos-server/chaos.example.toml`

- [ ] **Step 1: Write a failing test** in `config.rs` `mod tests`:

```rust
#[test]
fn forward_auth_defaults_off_with_authentik_headers() {
    let c = ForwardAuthConfig::default();
    assert!(c.secret.is_none());
    assert!(!c.enabled());
    assert_eq!(c.username_header, "x-authentik-username");
    assert_eq!(c.name_header, "x-authentik-name");
    assert_eq!(c.secret_header, "x-chaos-proxy-secret");
}
```

- [ ] **Step 2: Run, verify fail.** `cargo test -p chaos-server forward_auth_defaults -v`. Expected: FAIL.

- [ ] **Step 3: Add the type + field.** In `config.rs`:

```rust
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(default)]
pub struct ForwardAuthConfig {
    pub secret: Option<String>,
    pub username_header: String,
    pub name_header: String,
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
impl ForwardAuthConfig {
    pub fn enabled(&self) -> bool { self.secret.is_some() }
}
```

Add `pub forward_auth: ForwardAuthConfig,` to `Config` (it has `#[serde(default)]`
on the struct per the map; if not, add `#[serde(default)]` on the field) and to
the `Config::default` constructor (`forward_auth: ForwardAuthConfig::default()`).

- [ ] **Step 4: Document in `chaos.example.toml`:**

```toml
# [forward_auth]
# Trust an authenticating reverse proxy (authentik via traefik). OFF unless
# `secret` is set. The proxy MUST send `secret_header` with this exact value on
# every request; a request lacking it is never trusted, so a direct/tailnet
# client cannot forge an identity. The proxy also forwards `username_header`
# (the identity) and `name_header` (display name for auto-provisioned users).
# secret = "a-long-random-string"
# username_header = "X-authentik-username"
# name_header = "X-authentik-name"
# secret_header = "X-Chaos-Proxy-Secret"
```

- [ ] **Step 5: Run tests + clippy + fmt.** Expected: green.

- [ ] **Step 6: Commit** `feat(config): ForwardAuthConfig (off unless a shared secret is set)`.

---

### Task A2: DB resolve-or-provision

**Files:** `crates/chaos-server/src/db_auth.rs`

Note: `user_by_username(&self, username) -> Result<User>` already exists and
ERRORS when the user is absent (not `Option`). `create_user(username, display_name, password_hash) -> User` also exists.

- [ ] **Step 1: Write a failing test.**

```rust
#[tokio::test]
async fn user_by_username_or_create_is_idempotent() {
    let db = Db::in_memory().await.unwrap();   // use the crate's real in-memory helper
    let a = db.user_by_username_or_create("so", "So Balem").await.unwrap();
    assert_eq!(a.username, "so");
    assert_eq!(a.display_name, "So Balem");
    let b = db.user_by_username_or_create("so", "ignored on second call").await.unwrap();
    assert_eq!(a.id, b.id);
    assert_eq!(b.display_name, "So Balem"); // not overwritten
}
```

> Use the crate's actual in-memory test-db constructor (grep `db_auth.rs` /
> `db_calendar.rs` tests — it's `Db::in_memory()` per the views work).

- [ ] **Step 2: Run, verify fail.** `cargo test -p chaos-server user_by_username_or_create -v`. Expected: FAIL.

- [ ] **Step 3: Implement.**

```rust
impl Db {
    /// Resolve a user by username; create one (empty password_hash → external
    /// identity only, no password login) if absent. `display_name` used only
    /// on creation.
    pub async fn user_by_username_or_create(
        &self,
        username: &str,
        display_name: &str,
    ) -> Result<User> {
        match self.user_by_username(username).await {
            Ok(user) => Ok(user),
            Err(_) => self.create_user(username, display_name, "").await,
        }
    }
}
```

> If `user_by_username`'s error type distinguishes "not found" from real DB
> errors, match specifically on not-found and propagate other errors. If it just
> returns a generic error on missing row, the `Err(_) => create` above is
> acceptable (a transient DB error would surface as a failed create). Prefer the
> specific match if the error enum allows it.

- [ ] **Step 4: Verify empty-hash login rejection.** Add a test that a user with
an empty `password_hash` cannot log in:

```rust
#[test]
fn verify_login_rejects_empty_hash() {
    assert!(!crate::auth::verify_login(Some(""), "anything"));
}
```

If this panics (argon2 parsing `""`), guard `verify_login` to treat an empty
stored hash like `None`:

```rust
Some(hash) if !hash.is_empty() => verify_password(password, hash),
_ => { let _ = verify_password(password, dummy_hash()); false }
```

- [ ] **Step 5: Run tests + clippy.** Expected: green.

- [ ] **Step 6: Commit** `feat(db): resolve-or-provision user by username; reject empty-hash login`.

---

### Task A3: Extractor forward-auth branch

**Files:** `crates/chaos-server/src/auth.rs`

- [ ] **Step 1: Write failing tests** for the pure resolver `forward_auth_user`.
It needs an `AppState` (or just the `ForwardAuthConfig` + `Db`) — factor the
header logic so it's testable. Prefer a signature taking what it needs:
`async fn forward_auth_user(headers: &HeaderMap, cfg: &ForwardAuthConfig, db: &Db) -> Result<Option<User>, ApiError>` and have the extractor pass `&state.config.forward_auth, &state.db`.

```rust
#[tokio::test]
async fn forward_auth_disabled_returns_none() {
    let db = Db::in_memory().await.unwrap();
    let cfg = ForwardAuthConfig::default(); // secret None
    let mut h = HeaderMap::new();
    h.insert("x-authentik-username", "so".parse().unwrap());
    assert!(forward_auth_user(&h, &cfg, &db).await.unwrap().is_none());
}

#[tokio::test]
async fn forward_auth_requires_matching_secret_then_provisions() {
    let db = Db::in_memory().await.unwrap();
    let cfg = ForwardAuthConfig { secret: Some("s3cret".into()), ..Default::default() };
    // wrong secret → None
    let mut bad = HeaderMap::new();
    bad.insert("x-chaos-proxy-secret", "nope".parse().unwrap());
    bad.insert("x-authentik-username", "so".parse().unwrap());
    assert!(forward_auth_user(&bad, &cfg, &db).await.unwrap().is_none());
    // right secret + username + name → provisions
    let mut ok = HeaderMap::new();
    ok.insert("x-chaos-proxy-secret", "s3cret".parse().unwrap());
    ok.insert("x-authentik-username", "so".parse().unwrap());
    ok.insert("x-authentik-name", "So Balem".parse().unwrap());
    let user = forward_auth_user(&ok, &cfg, &db).await.unwrap().unwrap();
    assert_eq!(user.username, "so");
    assert_eq!(user.display_name, "So Balem");
    // missing name → display falls back to username
    let mut noname = HeaderMap::new();
    noname.insert("x-chaos-proxy-secret", "s3cret".parse().unwrap());
    noname.insert("x-authentik-username", "ann".parse().unwrap());
    let u2 = forward_auth_user(&noname, &cfg, &db).await.unwrap().unwrap();
    assert_eq!(u2.display_name, "ann");
}
```

- [ ] **Step 2: Run, verify fail.** `cargo test -p chaos-server forward_auth -v`. Expected: FAIL.

- [ ] **Step 3: Implement `forward_auth_user`** (see spec §A3 for the body). Key
points: `cfg.secret` None → None; secret header must equal `secret`; username
header required + non-empty; name header → display, else username;
`db.user_by_username_or_create(username, display)`.

- [ ] **Step 4: Rewrite the extractor** to try token then forward-auth:

```rust
async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
    if let Some(token) = request_token(&parts.headers)
        && let Ok(user) = state.db.user_by_session(&token_hash(&token)).await
    {
        return Ok(AuthUser(user));
    }
    if let Some(user) =
        forward_auth_user(&parts.headers, &state.config.forward_auth, &state.db).await?
    {
        return Ok(AuthUser(user));
    }
    Err(ApiError::Unauthorized)
}
```

Also update `optional_user_id` to fall back to forward-auth on the None path so
`search`/attribution events attribute correctly:

```rust
pub async fn optional_user_id(state: &AppState, headers: &HeaderMap) -> Option<Uuid> {
    if let Some(token) = request_token(headers)
        && let Ok(user) = state.db.user_by_session(&token_hash(&token)).await
    {
        return Some(user.id);
    }
    forward_auth_user(headers, &state.config.forward_auth, &state.db)
        .await
        .ok()
        .flatten()
        .map(|u| u.id)
}
```

- [ ] **Step 5: Extractor precedence test.** Build a request with BOTH a valid
session token AND forward-auth headers; assert the token user wins. (If the
crate has no full-request harness, test the two resolvers directly: a valid
token resolves via `user_by_session`, and the extractor tries it first — assert
by ordering/logic. At minimum, `forward_auth_user` tests from Step 1 + a note
that the extractor calls token-first cover it.)

- [ ] **Step 6: Run tests + clippy + fmt.** Expected: green.

- [ ] **Step 7: Commit** `feat(auth): trust forwarded authentik identity behind a shared secret`.

---

### Task A4: Deployment docs

**Files:** `docs/deployment.md`

- [ ] **Step 1: Add a "Behind authentik (forward-auth)" subsection** documenting:
`forward_auth.secret` enables it; the traefik middleware must forward
`X-authentik-username`/`X-authentik-name` from the outpost AND stamp
`X-Chaos-Proxy-Secret: <secret>` on requests to chaos, stripping any
client-supplied copy; the app authenticates to authentik with an app-password
(Basic) — see Plan B; a direct/tailnet route stays safe because without the
secret header chaos falls back to its own login. Keep it concise and concrete.

- [ ] **Step 2: Commit** `docs: deploying chaos behind authentik forward-auth`.

---

## Self-review notes
- Spec coverage: A1 config; A2 provision + empty-hash; A3 extractor + optional_user_id; A4 docs. Covers §Plan A.
- Type consistency: `ForwardAuthConfig{secret:Option<String>,username_header,name_header,secret_header}` + `enabled()`; `user_by_username_or_create(&str,&str)->Result<User>`; `forward_auth_user(&HeaderMap,&ForwardAuthConfig,&Db)->Result<Option<User>,ApiError>`. Consistent across tasks.
- Security: forward-auth off unless `secret` set; secret header must match; empty-hash users can't password-login. Token path unchanged and wins.
