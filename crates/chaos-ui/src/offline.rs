//! Offline support: the app-wide connectivity state and the cache-first
//! read path. Modeled on yomu's offline core.
//!
//! Connectivity is decided by the health probe ALONE — never
//! `navigator.onLine` (a device away from a self-hosted server has
//! connectivity but no route home), and never a per-request success
//! (only the probe promotes to Online). The first failed server request
//! downgrades Online → Offline.

// Wired into App/ServerGate in the next commit.
#![allow(dead_code)]

use chaos_client::{ChaosClient, ClientError};
use leptos::prelude::*;
use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Connectivity {
    /// Boot: the first health probe hasn't answered yet.
    Checking,
    Online,
    Offline,
}

pub(crate) fn use_connectivity() -> RwSignal<Connectivity> {
    use_context::<RwSignal<Connectivity>>().expect("Connectivity provided by App")
}

const CACHE_PREFIX: &str = "chaos-cache:";
const SERVERS_SEEN_KEY: &str = "chaos-servers-seen";

pub(crate) fn cache_put<T: Serialize>(key: &str, value: &T) {
    if let (Some(storage), Ok(json)) = (crate::local_storage(), serde_json::to_string(value)) {
        let _ = storage.set_item(&format!("{CACHE_PREFIX}{key}"), &json);
    }
}

pub(crate) fn cache_get<T: DeserializeOwned>(key: &str) -> Option<T> {
    let raw = crate::local_storage()?
        .get_item(&format!("{CACHE_PREFIX}{key}"))
        .ok()??;
    serde_json::from_str(&raw).ok()
}

/// The one cache-first read path. Offline (or still checking) with a cached
/// copy: serve it immediately, zero network. Online: fetch; a success
/// overwrites the cache (that's the only invalidation — no TTL); a
/// *transport* failure downgrades connectivity and falls back to the cache.
/// API errors (401, 404, validation) pass through untouched: the server
/// answered, so this is not a connectivity problem and stale data would be
/// wrong.
///
/// Returns `(value, stale)` — `stale` means "came from the cache".
pub(crate) async fn cached<T, Fut>(
    conn: RwSignal<Connectivity>,
    key: &str,
    fetch: Fut,
) -> Result<(T, bool), ClientError>
where
    T: Serialize + DeserializeOwned,
    Fut: Future<Output = Result<T, ClientError>>,
{
    if conn.get_untracked() != Connectivity::Online
        && let Some(hit) = cache_get::<T>(key)
    {
        return Ok((hit, true));
    }
    match fetch.await {
        Ok(value) => {
            cache_put(key, &value);
            Ok((value, false))
        }
        Err(err) if is_connectivity_error(&err) => {
            // Downgrade only — promotion to Online is the probe's job.
            if conn.get_untracked() == Connectivity::Online {
                conn.set(Connectivity::Offline);
            }
            match cache_get::<T>(key) {
                Some(hit) => Ok((hit, true)),
                None => Err(err),
            }
        }
        Err(err) => Err(err),
    }
}

/// Only failures to REACH the server say anything about connectivity.
fn is_connectivity_error(err: &ClientError) -> bool {
    matches!(err, ClientError::Transport(_))
}

// ---- "server seen" gate memory ----
// Distinguishes "misconfigured / never reached" (connect form) from "known
// server that is just offline right now" (cached UI + badge).

fn seen_list(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

pub(crate) fn server_seen(base: &str) -> bool {
    crate::local_storage()
        .and_then(|s| s.get_item(SERVERS_SEEN_KEY).ok().flatten())
        .is_some_and(|raw| seen_list(&raw).iter().any(|b| b == base))
}

pub(crate) fn mark_server_seen(base: &str) {
    let Some(storage) = crate::local_storage() else {
        return;
    };
    let raw = storage
        .get_item(SERVERS_SEEN_KEY)
        .ok()
        .flatten()
        .unwrap_or_default();
    let mut list = seen_list(&raw);
    if !list.iter().any(|b| b == base) {
        list.push(base.to_string());
        let _ = storage.set_item(SERVERS_SEEN_KEY, &list.join("\n"));
    }
}

/// One bounded health probe; the only code path that can set `Online`.
/// Returns whether the server answered.
pub(crate) async fn probe(client: &ChaosClient, conn: RwSignal<Connectivity>) -> bool {
    match client.health().await {
        Ok(health) => {
            crate::set_server_fahrenheit(health.fahrenheit);
            mark_server_seen(client.base().as_str());
            conn.set(Connectivity::Online);
            true
        }
        Err(_) => {
            conn.set(Connectivity::Offline);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_errors_are_connectivity_errors_api_errors_are_not() {
        assert!(is_connectivity_error(&ClientError::Transport(
            "connection refused".into()
        )));
        assert!(!is_connectivity_error(&ClientError::Api {
            status: 401,
            message: "who are you".into()
        }));
        assert!(!is_connectivity_error(&ClientError::Decode(
            "bad json".into()
        )));
    }

    #[test]
    fn seen_list_parses_and_ignores_blanks() {
        let raw = "http://zeus:4600/\n\n  http://other:4600/  \n";
        assert_eq!(seen_list(raw), ["http://zeus:4600/", "http://other:4600/"]);
        assert!(seen_list("").is_empty());
    }
}
