# App OIDC Authentication — Plan A (server)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let chaos-server authenticate a request by an authentik-issued OIDC access token, require authentication on every API route, and advertise to clients how to sign in.

**Architecture:** A new `oidc` module verifies RS256 JWTs locally against a cached JWKS fetched from the issuer's discovery document, maps `preferred_username` onto a chaos user with the same auto-provisioning rules `forward_auth_user` already uses, and becomes the first source consulted by the `AuthUser` extractor. Every handler that lacks `AuthUser` gains it, with `/health` and `/auth/login` as the only allowlisted routes, guarded by a route-coverage test. `/api/v1/health` grows an `auth` block so the app can self-configure.

**Tech Stack:** Rust 2024, axum 0.8, `jsonwebtoken` 9 (RS256 + JWKS), reqwest, sqlx/SQLite, `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-07-25-app-oidc-auth-design.md`

---

## Context for every task

- Repo root `/projects/rust/chaos`, branch `feat/app-oidc-auth` (already checked out). Work inside `nix develop` / direnv.
- **Commits:** unsigned, with both trailers:
  ```bash
  git -c commit.gpgsign=false commit -m "type: subject" \
    -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
    -m "Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP"
  ```
  Do not push. Never commit anything under `/etc/nixos`.
- **Never touch** `crates/chaos-desktop/gen/schemas/{android,mobile}-schema.json`.
- After any Rust change: `just check` (fmt, clippy `-D warnings`, wasm check) and `just test` must both pass before committing.
- Existing patterns to follow: `crates/chaos-server/src/auth.rs` holds every identity concern (`forward_auth_user`, `AuthUser`, `note_sso_login`); `ApiError::Unauthorized` is the 401; config lives in `crates/chaos-server/src/config.rs` with `#[serde(default)]` sub-structs like `ForwardAuthConfig`.
- **Security rule for this plan:** a malformed, expired or otherwise invalid Bearer JWT must return 401 — never fall through to a weaker identity source.

## File structure

| File | Responsibility |
| --- | --- |
| `crates/chaos-server/src/oidc.rs` (new) | Everything OIDC: config-driven discovery, JWKS cache, RS256 verification, claims type. No axum, no DB — pure verification + a fetcher. |
| `crates/chaos-server/src/auth.rs` | Gains `oidc_user()` (claims → chaos user) and a third branch in `AuthUser`. Identity policy stays in one file. |
| `crates/chaos-server/src/config.rs` | `OidcConfig` + `Config.oidc`. |
| `crates/chaos-domain/src/api.rs` | `HealthResponse.auth` and the `AuthAdvertisement`/`OidcAdvertisement` types (shared with the client). |
| `crates/chaos-server/src/api/services.rs` | `health` advertises the auth block. |
| `crates/chaos-server/src/api/*.rs` | 27 handlers gain `AuthUser`. |
| `crates/chaos-server/src/api/mod.rs` | Route-coverage test. |
| `crates/chaos-server/src/state.rs` | Holds the shared `Jwks` cache. |

---

### Task 1: `[oidc]` config

**Files:**
- Modify: `crates/chaos-server/src/config.rs`
- Modify: `crates/chaos-server/chaos.example.toml`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block at the bottom of
`crates/chaos-server/src/config.rs`:

```rust
    #[test]
    fn oidc_is_disabled_until_both_issuer_and_client_id_are_set() {
        let none = OidcConfig::default();
        assert!(!none.enabled());

        let half = OidcConfig {
            issuer: Some("https://auth.example/application/o/chaos-app/".into()),
            ..Default::default()
        };
        assert!(!half.enabled());

        let full = OidcConfig {
            issuer: Some("https://auth.example/application/o/chaos-app/".into()),
            client_id: Some("abc123".into()),
        };
        assert!(full.enabled());
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo nextest run -p chaos-server oidc_is_disabled`
Expected: FAIL to compile — `cannot find type 'OidcConfig' in this scope`.

- [ ] **Step 3: Implement**

In `crates/chaos-server/src/config.rs`, add next to `ForwardAuthConfig`:

```rust
/// OIDC (authentik) access tokens presented by the apps as
/// `Authorization: Bearer`. Off unless both values are set — a half-configured
/// issuer must not silently accept anything.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct OidcConfig {
    /// Issuer URL, exactly as it appears in the `iss` claim — authentik's is
    /// `https://<auth host>/application/o/<app slug>/` (trailing slash).
    /// Discovery is this URL + `.well-known/openid-configuration`.
    pub issuer: Option<String>,
    /// The public client's id; checked against the token's `aud`.
    pub client_id: Option<String>,
}

impl OidcConfig {
    pub fn enabled(&self) -> bool {
        self.issuer.as_ref().is_some_and(|s| !s.trim().is_empty())
            && self.client_id.as_ref().is_some_and(|s| !s.trim().is_empty())
    }

    /// The OIDC discovery document URL for this issuer.
    pub fn discovery_url(&self) -> Option<String> {
        let issuer = self.issuer.as_ref()?.trim_end_matches('/');
        Some(format!("{issuer}/.well-known/openid-configuration"))
    }
}
```

And add the field to `Config` (next to `forward_auth`):

```rust
    /// OIDC bearer-token auth for the apps; see OidcConfig.
    #[serde(default)]
    pub oidc: OidcConfig,
```

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo nextest run -p chaos-server oidc_is_disabled`
Expected: PASS.

- [ ] **Step 5: Document it in the example config**

Append to `crates/chaos-server/chaos.example.toml`:

```toml
# OIDC access tokens from the mobile/desktop apps (authentik). Both values
# must be set for it to take effect. The issuer is what appears in the token's
# `iss` claim — authentik uses a trailing slash. Requires a provider whose
# signing key is an RSA certificate, so the JWKS is non-empty.
# [oidc]
# issuer = "https://auth.example.com/application/o/chaos-app/"
# client_id = "your-public-client-id"
```

- [ ] **Step 6: Commit**

```bash
git add crates/chaos-server/src/config.rs crates/chaos-server/chaos.example.toml
git -c commit.gpgsign=false commit -m "feat(server): add the oidc config block" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP"
```

---

### Task 2: JWKS cache and RS256 verification

The verifier is deliberately split from the fetcher so the hard part (claim and
signature validation) is testable without a network or a live authentik.

**Files:**
- Create: `crates/chaos-server/src/oidc.rs`
- Modify: `crates/chaos-server/src/lib.rs` or `main.rs` (whichever declares the modules — check with `grep -n "^mod \|^pub mod " crates/chaos-server/src/*.rs`)
- Modify: `crates/chaos-server/Cargo.toml`

- [ ] **Step 1: Add the dependencies**

In `crates/chaos-server/Cargo.toml`, add to `[dependencies]`:

```toml
jsonwebtoken = "9"
```

and add a `[dev-dependencies]` section (or extend the existing one):

```toml
[dev-dependencies]
rsa = { version = "0.9", features = ["pem"] }
rand = "0.8"
base64 = "0.22"
```

Run: `cargo check -p chaos-server`
Expected: compiles.

- [ ] **Step 2: Write the failing tests**

Create `crates/chaos-server/src/oidc.rs` with the API surface and tests, leaving
the bodies unimplemented:

```rust
//! OIDC access-token verification for the apps.
//!
//! Tokens are verified locally against a JWKS cached from the issuer's
//! discovery document: no per-request round trip to authentik, and a brief
//! authentik outage doesn't lock out clients that already hold a token.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::config::OidcConfig;

/// How long a fetched key set is trusted before a refetch is attempted.
const JWKS_TTL: Duration = Duration::from_secs(3600);

/// The claims chaos needs from an authentik access token.
#[derive(Debug, Clone, Deserialize)]
pub struct Claims {
    /// authentik username — the mapping key onto a chaos account.
    pub preferred_username: String,
    /// Display name; absent for users who never set one.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum OidcError {
    #[error("oidc is not configured")]
    Disabled,
    #[error("token rejected: {0}")]
    Invalid(String),
    #[error("signing key {0} is not in the key set")]
    UnknownKid(String),
    #[error("could not reach the issuer: {0}")]
    Discovery(String),
}

/// Decoding keys by `kid`, with the time they were fetched.
pub struct Jwks {
    keys: RwLock<Option<CachedKeys>>,
    http: reqwest::Client,
}

struct CachedKeys {
    by_kid: HashMap<String, DecodingKey>,
    fetched: Instant,
}

impl Jwks {
    pub fn new(http: reqwest::Client) -> Arc<Self> {
        Arc::new(Self {
            keys: RwLock::new(None),
            http,
        })
    }

    /// Verify a token, fetching (or refreshing) the key set if needed.
    pub async fn verify(&self, token: &str, cfg: &OidcConfig) -> Result<Claims, OidcError> {
        unimplemented!("Jwks::verify")
    }
}

/// Verify `token` against an already-resolved key set. The whole of the
/// validation policy lives here, with no I/O, so it is directly testable.
pub fn verify_with_keys(
    token: &str,
    keys: &HashMap<String, DecodingKey>,
    cfg: &OidcConfig,
) -> Result<Claims, OidcError> {
    unimplemented!("verify_with_keys")
}

/// Parse a JWKS document into decoding keys by `kid`, skipping entries that
/// aren't RSA signing keys.
pub fn parse_jwks(document: &str) -> Result<HashMap<String, DecodingKey>, OidcError> {
    unimplemented!("parse_jwks")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use jsonwebtoken::{Algorithm, EncodingKey, Header};
    use rsa::pkcs1::EncodeRsaPrivateKey;
    use rsa::traits::PublicKeyParts;
    use rsa::{RsaPrivateKey, RsaPublicKey};
    use serde::Serialize;

    use super::*;

    const KID: &str = "test-key";
    const ISSUER: &str = "https://auth.example/application/o/chaos-app/";
    const CLIENT_ID: &str = "chaos-app-client";

    fn config() -> OidcConfig {
        OidcConfig {
            issuer: Some(ISSUER.into()),
            client_id: Some(CLIENT_ID.into()),
        }
    }

    #[derive(Serialize)]
    struct TestClaims {
        iss: String,
        aud: String,
        exp: i64,
        preferred_username: String,
        name: String,
    }

    /// One RSA keypair: the PEM for signing, and a JWKS document for the
    /// verifier — the same shape authentik publishes.
    fn keypair() -> (EncodingKey, String) {
        let mut rng = rand::thread_rng();
        let private = RsaPrivateKey::new(&mut rng, 2048).expect("generate key");
        let public = RsaPublicKey::from(&private);
        let pem = private.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).expect("pem");
        let encoding = EncodingKey::from_rsa_pem(pem.as_bytes()).expect("encoding key");
        let n = URL_SAFE_NO_PAD.encode(public.n().to_bytes_be());
        let e = URL_SAFE_NO_PAD.encode(public.e().to_bytes_be());
        let jwks = format!(
            r#"{{"keys":[{{"kty":"RSA","use":"sig","alg":"RS256","kid":"{KID}","n":"{n}","e":"{e}"}}]}}"#
        );
        (encoding, jwks)
    }

    fn token(encoding: &EncodingKey, claims: TestClaims) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(KID.into());
        jsonwebtoken::encode(&header, &claims, encoding).expect("sign")
    }

    fn valid_claims() -> TestClaims {
        TestClaims {
            iss: ISSUER.into(),
            aud: CLIENT_ID.into(),
            exp: (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp(),
            preferred_username: "tibo".into(),
            name: "Tibo".into(),
        }
    }

    #[test]
    fn a_valid_token_yields_its_claims() {
        let (encoding, jwks) = keypair();
        let keys = parse_jwks(&jwks).expect("parse");
        let claims = verify_with_keys(&token(&encoding, valid_claims()), &keys, &config())
            .expect("valid token");
        assert_eq!(claims.preferred_username, "tibo");
        assert_eq!(claims.name.as_deref(), Some("Tibo"));
    }

    #[test]
    fn an_expired_token_is_rejected() {
        let (encoding, jwks) = keypair();
        let keys = parse_jwks(&jwks).expect("parse");
        let expired = TestClaims {
            exp: (chrono::Utc::now() - chrono::Duration::hours(1)).timestamp(),
            ..valid_claims()
        };
        assert!(matches!(
            verify_with_keys(&token(&encoding, expired), &keys, &config()),
            Err(OidcError::Invalid(_))
        ));
    }

    #[test]
    fn a_token_from_another_issuer_is_rejected() {
        let (encoding, jwks) = keypair();
        let keys = parse_jwks(&jwks).expect("parse");
        let wrong = TestClaims {
            iss: "https://evil.example/application/o/chaos-app/".into(),
            ..valid_claims()
        };
        assert!(matches!(
            verify_with_keys(&token(&encoding, wrong), &keys, &config()),
            Err(OidcError::Invalid(_))
        ));
    }

    #[test]
    fn a_token_for_another_audience_is_rejected() {
        let (encoding, jwks) = keypair();
        let keys = parse_jwks(&jwks).expect("parse");
        let wrong = TestClaims {
            aud: "some-other-client".into(),
            ..valid_claims()
        };
        assert!(matches!(
            verify_with_keys(&token(&encoding, wrong), &keys, &config()),
            Err(OidcError::Invalid(_))
        ));
    }

    /// A token signed by a different key with the same `kid` — the signature
    /// check is what catches it.
    #[test]
    fn a_token_signed_by_a_stranger_is_rejected() {
        let (_, jwks) = keypair();
        let (other_encoding, _) = keypair();
        let keys = parse_jwks(&jwks).expect("parse");
        assert!(matches!(
            verify_with_keys(&token(&other_encoding, valid_claims()), &keys, &config()),
            Err(OidcError::Invalid(_))
        ));
    }

    #[test]
    fn an_unknown_kid_is_reported_as_such() {
        let (encoding, _) = keypair();
        let empty: HashMap<String, DecodingKey> = HashMap::new();
        assert!(matches!(
            verify_with_keys(&token(&encoding, valid_claims()), &empty, &config()),
            Err(OidcError::UnknownKid(_))
        ));
    }

    #[test]
    fn garbage_is_rejected_without_panicking() {
        let (_, jwks) = keypair();
        let keys = parse_jwks(&jwks).expect("parse");
        for junk in ["", "not-a-token", "a.b.c"] {
            assert!(verify_with_keys(junk, &keys, &config()).is_err());
        }
    }

    #[test]
    fn verification_without_configuration_is_disabled() {
        let (encoding, jwks) = keypair();
        let keys = parse_jwks(&jwks).expect("parse");
        assert!(matches!(
            verify_with_keys(
                &token(&encoding, valid_claims()),
                &keys,
                &OidcConfig::default()
            ),
            Err(OidcError::Disabled)
        ));
    }

    #[test]
    fn parse_jwks_skips_non_rsa_entries() {
        let doc = r#"{"keys":[
            {"kty":"oct","kid":"symmetric","k":"c2VjcmV0"},
            {"kty":"RSA","use":"sig","alg":"RS256","kid":"good","n":"sXchDaQe","e":"AQAB"}
        ]}"#;
        let keys = parse_jwks(doc).expect("parse");
        assert_eq!(keys.len(), 1);
        assert!(keys.contains_key("good"));
    }

    /// authentik's proxy providers publish `{}` (they sign HS256). An empty
    /// key set must parse cleanly and simply verify nothing.
    #[test]
    fn parse_jwks_accepts_an_empty_document() {
        assert!(parse_jwks("{}").expect("parse").is_empty());
    }
}
```

Register the module. Check where modules are declared first:

Run: `grep -n "^mod \|^pub mod " crates/chaos-server/src/main.rs crates/chaos-server/src/lib.rs 2>/dev/null | head`

Then add `mod oidc;` in alphabetical position among the existing declarations.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo nextest run -p chaos-server oidc::`
Expected: 10 tests run, all FAIL with `not implemented: verify_with_keys` /
`not implemented: parse_jwks`.

- [ ] **Step 4: Implement `parse_jwks`**

Replace the `parse_jwks` body in `crates/chaos-server/src/oidc.rs`:

```rust
#[derive(Deserialize)]
struct JwksDocument {
    #[serde(default)]
    keys: Vec<JwkEntry>,
}

#[derive(Deserialize)]
struct JwkEntry {
    kty: String,
    kid: Option<String>,
    n: Option<String>,
    e: Option<String>,
}

pub fn parse_jwks(document: &str) -> Result<HashMap<String, DecodingKey>, OidcError> {
    let parsed: JwksDocument =
        serde_json::from_str(document).map_err(|e| OidcError::Discovery(e.to_string()))?;
    let mut by_kid = HashMap::new();
    for entry in parsed.keys {
        // Only RSA signing keys are usable here; authentik also publishes
        // symmetric entries for providers that sign HS256.
        if entry.kty != "RSA" {
            continue;
        }
        let (Some(kid), Some(n), Some(e)) = (entry.kid, entry.n, entry.e) else {
            continue;
        };
        if let Ok(key) = DecodingKey::from_rsa_components(&n, &e) {
            by_kid.insert(kid, key);
        }
    }
    Ok(by_kid)
}
```

- [ ] **Step 5: Implement `verify_with_keys`**

```rust
pub fn verify_with_keys(
    token: &str,
    keys: &HashMap<String, DecodingKey>,
    cfg: &OidcConfig,
) -> Result<Claims, OidcError> {
    let (Some(issuer), Some(client_id)) = (cfg.issuer.as_ref(), cfg.client_id.as_ref()) else {
        return Err(OidcError::Disabled);
    };
    if !cfg.enabled() {
        return Err(OidcError::Disabled);
    }
    let header =
        jsonwebtoken::decode_header(token).map_err(|e| OidcError::Invalid(e.to_string()))?;
    let kid = header.kid.ok_or_else(|| OidcError::Invalid("no kid".into()))?;
    let key = keys.get(&kid).ok_or(OidcError::UnknownKid(kid))?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[issuer.as_str()]);
    validation.set_audience(&[client_id.as_str()]);
    // exp is required and checked by default; be explicit so a future
    // jsonwebtoken default change can't silently relax it.
    validation.validate_exp = true;
    validation.validate_nbf = true;

    jsonwebtoken::decode::<Claims>(token, key, &validation)
        .map(|data| data.claims)
        .map_err(|e| OidcError::Invalid(e.to_string()))
}
```

- [ ] **Step 6: Implement `Jwks::verify`**

```rust
impl Jwks {
    /// Verify a token, fetching the key set on first use and refetching once
    /// when a `kid` is unknown (authentik rotates signing keys) or the cache
    /// has aged past JWKS_TTL.
    pub async fn verify(&self, token: &str, cfg: &OidcConfig) -> Result<Claims, OidcError> {
        if !cfg.enabled() {
            return Err(OidcError::Disabled);
        }
        let cached = {
            let guard = self.keys.read().await;
            match guard.as_ref() {
                Some(cached) if cached.fetched.elapsed() < JWKS_TTL => Some(cached.by_kid.clone()),
                _ => None,
            }
        };
        if let Some(keys) = cached {
            match verify_with_keys(token, &keys, cfg) {
                // An unknown kid is the one error worth a refetch: everything
                // else is a verdict about the token, not about our key set.
                Err(OidcError::UnknownKid(_)) => {}
                other => return other,
            }
        }
        let keys = self.refresh(cfg).await?;
        verify_with_keys(token, &keys, cfg)
    }

    /// Fetch the discovery document, then the JWKS it points at, and cache.
    async fn refresh(&self, cfg: &OidcConfig) -> Result<HashMap<String, DecodingKey>, OidcError> {
        let discovery_url = cfg.discovery_url().ok_or(OidcError::Disabled)?;
        let discovery: DiscoveryDocument = self
            .http
            .get(&discovery_url)
            .send()
            .await
            .map_err(|e| OidcError::Discovery(e.to_string()))?
            .json()
            .await
            .map_err(|e| OidcError::Discovery(e.to_string()))?;
        let document = self
            .http
            .get(&discovery.jwks_uri)
            .send()
            .await
            .map_err(|e| OidcError::Discovery(e.to_string()))?
            .text()
            .await
            .map_err(|e| OidcError::Discovery(e.to_string()))?;
        let by_kid = parse_jwks(&document)?;
        *self.keys.write().await = Some(CachedKeys {
            by_kid: by_kid.clone(),
            fetched: Instant::now(),
        });
        Ok(by_kid)
    }
}

#[derive(Deserialize)]
struct DiscoveryDocument {
    jwks_uri: String,
}
```

`DecodingKey` is `Clone`, so the cached map can be cloned out from under the
read lock rather than held across an await.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo nextest run -p chaos-server oidc::`
Expected: 10 tests pass.

Run: `just check`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add crates/chaos-server/src/oidc.rs crates/chaos-server/Cargo.toml Cargo.lock \
        crates/chaos-server/src/main.rs crates/chaos-server/src/lib.rs
git -c commit.gpgsign=false commit -m "feat(server): verify authentik OIDC access tokens against a cached JWKS" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP"
```

(Adjust the `git add` list to the module file that actually declares `mod oidc;`.)

---

### Task 3: Map claims to a chaos user, and make `AuthUser` accept a Bearer JWT

**Files:**
- Modify: `crates/chaos-server/src/state.rs`
- Modify: `crates/chaos-server/src/auth.rs`

- [ ] **Step 1: Put the JWKS cache in `AppState`**

Read `crates/chaos-server/src/state.rs` first (`AppState::new` is called from
tests as `AppState::new(Config::default(), db)`, so its signature must not
change). Add a field:

```rust
    /// Cached OIDC signing keys, shared by every request (see oidc.rs).
    pub jwks: std::sync::Arc<crate::oidc::Jwks>,
```

and initialize it in `AppState::new` with the state's existing reqwest client if
there is one, otherwise `reqwest::Client::new()`:

```rust
        jwks: crate::oidc::Jwks::new(reqwest::Client::new()),
```

Run: `cargo check -p chaos-server`
Expected: compiles.

- [ ] **Step 2: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `crates/chaos-server/src/auth.rs`:

```rust
    #[tokio::test]
    async fn oidc_claims_provision_then_reuse_one_account() {
        let db = crate::db::Db::in_memory().await.unwrap();
        let claims = crate::oidc::Claims {
            preferred_username: "tibo".into(),
            name: Some("Tibo".into()),
        };

        let first = oidc_user(&claims, &db).await.expect("provision");
        assert_eq!(first.username, "tibo");
        assert_eq!(first.display_name, "Tibo");

        let second = oidc_user(&claims, &db).await.expect("reuse");
        assert_eq!(second.id, first.id, "the same authentik user is one account");
    }

    #[tokio::test]
    async fn oidc_claims_update_a_changed_display_name() {
        let db = crate::db::Db::in_memory().await.unwrap();
        let _ = oidc_user(
            &crate::oidc::Claims {
                preferred_username: "tibo".into(),
                name: Some("Tibo".into()),
            },
            &db,
        )
        .await
        .expect("provision");

        let renamed = oidc_user(
            &crate::oidc::Claims {
                preferred_username: "tibo".into(),
                name: Some("Thibaud".into()),
            },
            &db,
        )
        .await
        .expect("reuse");
        assert_eq!(renamed.display_name, "Thibaud");
    }

    /// No `name` claim: fall back to the username rather than storing an
    /// empty display name.
    #[tokio::test]
    async fn oidc_claims_without_a_name_fall_back_to_the_username() {
        let db = crate::db::Db::in_memory().await.unwrap();
        let user = oidc_user(
            &crate::oidc::Claims {
                preferred_username: "akadmin".into(),
                name: None,
            },
            &db,
        )
        .await
        .expect("provision");
        assert_eq!(user.display_name, "akadmin");
    }
```

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo nextest run -p chaos-server oidc_claims`
Expected: FAIL to compile — `cannot find function 'oidc_user'`.

- [ ] **Step 4: Implement `oidc_user`**

Read `forward_auth_user` in `crates/chaos-server/src/auth.rs` first and mirror
its provisioning: it looks the user up by username, creates one when missing,
and updates the display name when it changed. Add below it:

```rust
/// Resolve (or provision) the chaos user an OIDC token identifies. Same rules
/// as `forward_auth_user`, so an authentik account maps onto one chaos account
/// whether it arrives through the browser's forward-auth headers or the app's
/// bearer token.
pub async fn oidc_user(claims: &crate::oidc::Claims, db: &crate::db::Db) -> anyhow::Result<User> {
    let username = claims.preferred_username.trim();
    let display = claims
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(username);
    provision_proxy_user(username, display, db).await
}
```

If `forward_auth_user` does the lookup/creation inline rather than through a
helper, extract that block into `provision_proxy_user(username, display, db)`
and call it from both — the two must not drift.

- [ ] **Step 5: Run them to verify they pass**

Run: `cargo nextest run -p chaos-server oidc_claims`
Expected: 3 tests pass. Also run `cargo nextest run -p chaos-server forward_auth`
and confirm the pre-existing forward-auth tests still pass after the extraction.

- [ ] **Step 6: Add the Bearer branch to `AuthUser`**

Replace the `from_request_parts` body in `crates/chaos-server/src/auth.rs`:

```rust
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        // 1. An OIDC access token from the apps. A token that is present but
        //    invalid is a 401 — never a fallthrough to a weaker source, which
        //    would let a junk bearer inherit the browser's proxy identity.
        if let Some(bearer) = bearer_token(&parts.headers)
            && state.config.oidc.enabled()
        {
            return match state.jwks.verify(bearer, &state.config.oidc).await {
                Ok(claims) => {
                    let user = oidc_user(&claims, &state.db)
                        .await
                        .map_err(|_| ApiError::Unauthorized)?;
                    note_sso_login(state, &user, &parts.headers).await;
                    Ok(AuthUser(user))
                }
                Err(_) => Err(ApiError::Unauthorized),
            };
        }
        // 2. chaos session (Bearer/cookie) — unchanged, wins when present.
        if let Some(token) = request_token(&parts.headers)
            && let Ok(user) = state.db.user_by_session(&token_hash(&token)).await
        {
            return Ok(AuthUser(user));
        }
        // 3. trusted forward-auth header (only when configured + secret matches).
        if let Some(user) =
            forward_auth_user(&parts.headers, &state.config.forward_auth, &state.db).await?
        {
            note_sso_login(state, &user, &parts.headers).await;
            return Ok(AuthUser(user));
        }
        Err(ApiError::Unauthorized)
    }
```

A chaos session token is also sent as `Authorization: Bearer`, so branch 1 must
only claim a token that actually looks like a JWT. Add:

```rust
/// The bearer value, but only when it looks like a JWT (three dot-separated
/// segments) — chaos's own session tokens travel the same header and must
/// still reach the session branch.
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let token = value.strip_prefix("Bearer ")?.trim();
    (token.split('.').count() == 3).then_some(token)
}
```

- [ ] **Step 7: Test the precedence**

Add to the same test module:

```rust
    #[test]
    fn only_jwt_shaped_bearers_are_treated_as_oidc_tokens() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer header.payload.signature".parse().unwrap(),
        );
        assert_eq!(bearer_token(&headers), Some("header.payload.signature"));

        // A chaos session token is opaque, not a JWT: it must fall through to
        // the session branch instead of being rejected as a bad OIDC token.
        let mut session = HeaderMap::new();
        session.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer 0123456789abcdef".parse().unwrap(),
        );
        assert_eq!(bearer_token(&session), None);

        assert_eq!(bearer_token(&HeaderMap::new()), None);
    }
```

Run: `cargo nextest run -p chaos-server`
Expected: all pass.

Run: `just check`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add crates/chaos-server/src/auth.rs crates/chaos-server/src/state.rs
git -c commit.gpgsign=false commit -m "feat(server): accept OIDC access tokens as an AuthUser source" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP"
```

---

### Task 4: `/api/v1/health` advertises how to sign in

**Files:**
- Modify: `crates/chaos-domain/src/api.rs`
- Modify: `crates/chaos-server/src/api/services.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/chaos-server/src/api/services.rs` (create the test module if the
file has none):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, OidcConfig};

    #[tokio::test]
    async fn health_advertises_oidc_when_configured() {
        let config = Config {
            oidc: OidcConfig {
                issuer: Some("https://auth.example/application/o/chaos-app/".into()),
                client_id: Some("client-123".into()),
            },
            ..Config::default()
        };
        let Json(body) = health(State(config)).await;
        let oidc = body.auth.expect("auth block").oidc.expect("oidc block");
        assert_eq!(oidc.issuer, "https://auth.example/application/o/chaos-app/");
        assert_eq!(oidc.client_id, "client-123");
        assert_eq!(
            oidc.authorize_url,
            "https://auth.example/application/o/authorize/"
        );
    }

    #[tokio::test]
    async fn health_omits_the_auth_block_without_oidc() {
        let Json(body) = health(State(Config::default())).await;
        assert!(body.auth.is_none());
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo nextest run -p chaos-server health_advertises`
Expected: FAIL to compile — `health` takes no arguments and `HealthResponse` has
no `auth` field.

- [ ] **Step 3: Add the shared types**

In `crates/chaos-domain/src/api.rs`, add the field to `HealthResponse`:

```rust
    /// How to authenticate against this server, when it wants more than a
    /// LAN connection. Absent means "no OIDC configured" — the apps then skip
    /// the sign-in step entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthAdvertisement>,
```

and the types below it:

```rust
/// The authentication methods a server offers. One optional member per
/// method, so adding one later doesn't break older clients.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthAdvertisement {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oidc: Option<OidcAdvertisement>,
}

/// Everything an app needs to start an authorization-code + PKCE flow. The
/// apps hardcode nothing about the identity provider: they learn it here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OidcAdvertisement {
    pub issuer: String,
    pub client_id: String,
    /// Derived from the issuer host rather than a second config knob —
    /// authentik serves one authorize endpoint for every provider.
    pub authorize_url: String,
}
```

- [ ] **Step 4: Advertise from the handler**

In `crates/chaos-server/src/api/services.rs`, take the config and build the
block:

```rust
pub async fn health(State(config): State<Config>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        fahrenheit: Some(locale_fahrenheit()),
        auth: auth_advertisement(&config),
    })
}

/// authentik serves `/application/o/authorize/` for every provider, so the
/// authorize URL is the issuer's origin plus that fixed path.
fn auth_advertisement(config: &Config) -> Option<AuthAdvertisement> {
    let issuer = config.oidc.issuer.as_ref()?.trim().to_string();
    let client_id = config.oidc.client_id.as_ref()?.trim().to_string();
    if !config.oidc.enabled() {
        return None;
    }
    let parsed = url::Url::parse(&issuer).ok()?;
    let authorize_url = format!(
        "{}://{}/application/o/authorize/",
        parsed.scheme(),
        parsed.host_str()?
    );
    Some(AuthAdvertisement {
        oidc: Some(OidcAdvertisement {
            issuer,
            client_id,
            authorize_url,
        }),
    })
}
```

The router currently calls `get(services::health)` with `AppState`. Axum's
`State` extractor needs `Config: FromRef<AppState>`; check whether
`crates/chaos-server/src/state.rs` already implements `FromRef` for `Config`. If
it does not, either add:

```rust
impl axum::extract::FromRef<AppState> for Config {
    fn from_ref(state: &AppState) -> Self {
        state.config.clone()
    }
}
```

(requires `Config: Clone`, which it already derives), or change the handler to
take `State(state): State<AppState>` and read `&state.config` — in which case
update the two tests above to build an `AppState` with `Db::in_memory()` the way
`api/mod.rs`'s existing test does.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo nextest run -p chaos-server health`
Expected: both tests pass.

Run: `just check && just test`
Expected: clean; the whole suite green (the wasm check covers chaos-ui, which
consumes `HealthResponse` — a new optional field is additive, so it compiles
unchanged).

- [ ] **Step 6: Commit**

```bash
git add crates/chaos-domain/src/api.rs crates/chaos-server/src/api/services.rs \
        crates/chaos-server/src/state.rs
git -c commit.gpgsign=false commit -m "feat(server): advertise the OIDC provider from /health" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP"
```

---

### Task 5: Require authentication on every API route

This is the task that makes the traefik Bearer-bypass safe. 27 handlers across
7 modules currently take no identity.

**Files:**
- Modify: `crates/chaos-server/src/api/{links,collections,widgets,search,home,services,icons}.rs`
- Modify: `crates/chaos-server/src/api/mod.rs` (route-coverage test)

- [ ] **Step 1: Write the failing coverage test**

Add to the `#[cfg(test)] mod tests` block in `crates/chaos-server/src/api/mod.rs`:

```rust
    /// Every route must require a signed-in user except the two that cannot:
    /// `/health` (liveness + the auth advertisement the apps read before they
    /// have a token) and `/auth/login` (how you get one).
    ///
    /// Hitting each route unauthenticated and asserting 401 is what makes this
    /// a real guard: a handler that forgets `AuthUser` fails here instead of
    /// silently serving data on the public domain.
    #[tokio::test]
    async fn every_route_requires_auth_except_the_allowlist() {
        use axum::body::Body;
        use axum::http::{Method, Request, StatusCode};
        use tower::ServiceExt;

        const ALLOWLISTED: [(&str, Method); 2] = [
            ("/api/v1/health", Method::GET),
            ("/api/v1/auth/login", Method::POST),
        ];

        // (path, method) for every route in the router. Keep in sync with
        // `router()` — a new route without an entry here is a review miss.
        let routes: Vec<(&str, Method)> = vec![
            ("/api/v1/auth/logout", Method::POST),
            ("/api/v1/auth/me", Method::GET),
            ("/api/v1/calendars", Method::GET),
            ("/api/v1/calendars", Method::POST),
            ("/api/v1/calendars/00000000-0000-0000-0000-000000000000", Method::PUT),
            ("/api/v1/calendars/00000000-0000-0000-0000-000000000000", Method::DELETE),
            ("/api/v1/calendar/events", Method::GET),
            ("/api/v1/calendar/refresh", Method::POST),
            ("/api/v1/events", Method::POST),
            ("/api/v1/events/00000000-0000-0000-0000-000000000000", Method::PUT),
            ("/api/v1/events/00000000-0000-0000-0000-000000000000", Method::DELETE),
            ("/api/v1/services", Method::GET),
            ("/api/v1/services/x/systemd", Method::POST),
            ("/api/v1/dashboard", Method::GET),
            ("/api/v1/widgets/x", Method::GET),
            ("/api/v1/widgets/x/systemd", Method::POST),
            ("/api/v1/posts/hackernews", Method::GET),
            ("/api/v1/posts/hackernews/views", Method::GET),
            ("/api/v1/posts/views", Method::POST),
            ("/api/v1/analytics/events", Method::POST),
            ("/api/v1/posts/hackernews/1/comments", Method::GET),
            ("/api/v1/home/sensors", Method::GET),
            ("/api/v1/home/lights", Method::GET),
            ("/api/v1/home/lights/x", Method::POST),
            ("/api/v1/home/temperature", Method::GET),
            ("/api/v1/icons/si:github", Method::GET),
            ("/api/v1/links", Method::GET),
            ("/api/v1/links", Method::POST),
            ("/api/v1/links/00000000-0000-0000-0000-000000000000", Method::GET),
            ("/api/v1/links/00000000-0000-0000-0000-000000000000", Method::PUT),
            ("/api/v1/links/00000000-0000-0000-0000-000000000000", Method::DELETE),
            ("/api/v1/links/00000000-0000-0000-0000-000000000000/archive", Method::GET),
            ("/api/v1/links/00000000-0000-0000-0000-000000000000/archive", Method::POST),
            ("/api/v1/collections", Method::GET),
            ("/api/v1/collections", Method::POST),
            ("/api/v1/collections/00000000-0000-0000-0000-000000000000", Method::PUT),
            ("/api/v1/collections/00000000-0000-0000-0000-000000000000", Method::DELETE),
            ("/api/v1/tags", Method::GET),
            ("/api/v1/search", Method::GET),
        ];

        for (path, method) in routes {
            let db = Db::in_memory().await.unwrap();
            let state = AppState::new(Config::default(), db).unwrap();
            let request = Request::builder()
                .method(method.clone())
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap();
            let status = router(state)
                .oneshot(request)
                .await
                .expect("infallible")
                .status();

            if ALLOWLISTED.contains(&(path, method.clone())) {
                assert_ne!(
                    status,
                    StatusCode::UNAUTHORIZED,
                    "{method} {path} is allowlisted but rejected the request"
                );
            } else {
                assert_eq!(
                    status,
                    StatusCode::UNAUTHORIZED,
                    "{method} {path} served an unauthenticated request"
                );
            }
        }
    }
```

`tower` is already a dependency of `chaos-server` (added for the static-asset
work), so `ServiceExt::oneshot` is available.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo nextest run -p chaos-server every_route_requires_auth`
Expected: FAIL, naming the first unauthenticated route (e.g.
`GET /api/v1/services served an unauthenticated request`).

- [ ] **Step 3: Add `AuthUser` to the unauthenticated handlers**

For each handler in `links.rs`, `collections.rs`, `widgets.rs`, `search.rs`,
`home.rs`, `services.rs` (except `health`) and `icons.rs`, add the extractor as
the **first** parameter, matching the existing style in `calendar.rs`:

```rust
pub async fn services(
    AuthUser(_user): AuthUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<ServiceStatus>>, ApiError> {
```

Use `AuthUser(_user)` where the identity isn't needed and `AuthUser(user)` where
it is. Two special cases:

- `links.rs` and `search.rs` call `optional_user_id` for attribution. Keep those
  calls — they still work — but the handler must now also take `AuthUser`, since
  attribution and authorization are different questions.
- `services::health` keeps no extractor: it is allowlisted.

Add `use crate::auth::AuthUser;` to each module that doesn't import it yet.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo nextest run -p chaos-server every_route_requires_auth`
Expected: PASS.

Run: `just test`
Expected: the whole suite green. Existing handler tests that call these
functions directly now need an `AuthUser(user)` argument — fix each by building
a user the way the tests in `views.rs` already do.

- [ ] **Step 5: Commit**

```bash
git add crates/chaos-server/src/api/
git -c commit.gpgsign=false commit -m "feat(server): require authentication on every API route" \
  -m "The public domain relied entirely on authentik's forward-auth to protect links, collections, widgets, search, home, services and icons. Letting bearer-bearing requests bypass forward-auth (Plan B) is only safe once the API authenticates them itself." \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP"
```

---

### Task 6: End-to-end check against a real token, and documentation

- [ ] **Step 1: Verify the whole suite**

Run: `just check && just test`
Expected: both clean.

- [ ] **Step 2: Prove verification works against real authentik metadata**

This checks the JWKS path against the live server without needing the new
provider to exist yet. From the repo root:

```bash
curl -s https://auth.zeus.balem.fr/application/o/chaos/.well-known/openid-configuration \
  | tr ',' '\n' | grep jwks_uri
curl -s https://auth.zeus.balem.fr/application/o/chaos/jwks/
```

Expected: the discovery document exposes `jwks_uri`, and the proxy provider's
JWKS is `{}` — which is exactly why the spec requires the new provider to use an
RSA signing key. Record both outputs in the commit message or the final report;
they are the evidence for the "RSA signing key" instruction the user has to
follow in the authentik UI.

- [ ] **Step 3: Document the deployment requirements**

Append to `docs/deployment.md` a section:

```markdown
## App authentication (OIDC)

The mobile and desktop apps authenticate with an authentik-issued OIDC access
token rather than the browser's forward-auth session, because a `SameSite=Lax`
proxy cookie never rides along on the apps' cross-origin API calls.

Server side, `[oidc]` in the chaos config (issuer + client_id) turns on bearer
verification. Tokens are checked locally against the issuer's JWKS, so the
provider **must** use an RSA signing key — authentik's proxy providers sign
HS256 and publish an empty `jwks/`, which verifies nothing.

Because the apps' requests bypass the forward-auth middleware in traefik, every
API route requires authentication in chaos itself; `/health` and `/auth/login`
are the only exceptions, and a route-coverage test enforces that.
```

- [ ] **Step 4: Commit**

```bash
git add docs/deployment.md
git -c commit.gpgsign=false commit -m "docs: record the OIDC server requirements" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP"
```

---

## Self-review

**Spec coverage (section C):**

| spec requirement | task |
| --- | --- |
| `[oidc]` config block | 1 |
| JWKS cache, RS256, iss/aud/exp/nbf | 2 |
| claims → user with forward-auth's provisioning rules | 3 |
| `AuthUser` precedence Bearer → session → forward-auth | 3 |
| invalid JWT is 401, never a fallthrough | 3 (Step 6 + the `bearer_token` test) |
| every route requires auth, allowlist `/health` + `/auth/login` | 5 |
| route-coverage regression test | 5 |
| `/health` advertises issuer/client_id/authorize_url | 4 |
| no auth block when unconfigured | 4 |

Sections A (authentik UI), B (traefik) and D (app) are out of scope here: A and
B are the user's to apply, D is Plan B.

**Deliberate ordering:** Task 5 (require auth) lands before any traefik change,
matching the spec's "C before B" rollout rule — deploying this plan alone
changes nothing for existing clients, because the browser still arrives with
forward-auth headers and the app still can't reach the API at all.

**Type consistency:** `OidcConfig{issuer, client_id}`, `Claims{preferred_username, name}`,
`OidcError::{Disabled, Invalid, UnknownKid, Discovery}`, `Jwks::verify`,
`verify_with_keys`, `parse_jwks`, `oidc_user`, `bearer_token`,
`AuthAdvertisement{oidc}`, `OidcAdvertisement{issuer, client_id, authorize_url}`
are used identically in every task that mentions them.
