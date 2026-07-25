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

/// Verify `token` against an already-resolved key set. The whole of the
/// validation policy lives here, with no I/O, so it is directly testable.
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
    let kid = header
        .kid
        .ok_or_else(|| OidcError::Invalid("no kid".into()))?;
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

/// Parse a JWKS document into decoding keys by `kid`, skipping entries that
/// aren't RSA signing keys.
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
        let pem = private
            .to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
            .expect("pem");
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
