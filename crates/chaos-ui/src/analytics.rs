//! Viewed-state + engagement analytics: the offline-capable outbox, the
//! optimistic overlay, the debounced/reconnect flush, and the `app_open`
//! throttle. The PURE helpers (throttle predicate, flag merge, row class) are
//! unit-tested here; the localStorage/timer/observer glue is browser-only.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use chaos_client::ChaosClient;
use chaos_domain::{
    EventItem, RecordEventsRequest, RecordViewsRequest, Source, ViewEvent, ViewEventItem, ViewFlags,
};
use chrono::{DateTime, Utc};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::offline::{Connectivity, use_connectivity};

const OUTBOX_KEY: &str = "chaos-view-outbox";
const EVENTS_KEY: &str = "chaos-event-outbox";
const APPOPEN_KEY: &str = "chaos-appopen-at";

/// Global optimistic overlay: (source-as-str, post_id) -> flags. Provided once
/// in App context so NewsPage rows and the reader share it. Rows read their
/// flags reactively, so an OR into the map instantly restyles the row.
#[derive(Clone, Copy)]
pub(crate) struct Overlay(pub RwSignal<HashMap<(String, String), ViewFlags>>);

thread_local! {
    /// The client + connectivity + persist flag, captured once at `App` boot
    /// (where the reactive owner + context exist) so the debounced/reconnect
    /// flush works from plain JS callbacks (the IntersectionObserver) that have
    /// no owner to `use_context` from.
    static FLUSH_CTX: RefCell<Option<(ChaosClient, RwSignal<Connectivity>, bool)>> =
        const { RefCell::new(None) };
    /// Trailing-debounce guard: one pending flush timer coalesces a burst of
    /// `record_*` calls.
    static FLUSH_SCHEDULED: Cell<bool> = const { Cell::new(false) };
}

/// Provide the overlay context and capture the flush context. Call once in App.
pub(crate) fn provide_overlay() {
    provide_context(Overlay(RwSignal::new(HashMap::new())));
    let client = crate::use_client();
    let conn = use_connectivity();
    let persist = crate::persist_token();
    FLUSH_CTX.with(|c| *c.borrow_mut() = Some((client, conn, persist)));
}

pub(crate) fn overlay() -> Overlay {
    use_context::<Overlay>().expect("Overlay provided by App")
}

/// Present in context only when viewed-state tracking is active (authed on
/// `/news` + reader). Its absence tells `post_row_view` to render plain rows.
/// Carries the currently shown source.
#[derive(Clone, Copy)]
pub(crate) struct ViewedState {
    // Carried as the declared context shape (and a seam for a future
    // desktop-widget adoption); rows key off `ViewedState`'s *presence* and
    // already receive the active source as a render argument.
    #[allow(dead_code)]
    pub source: Source,
}

fn now_utc() -> DateTime<Utc> {
    Utc::now()
}

fn read_vec<T: serde::de::DeserializeOwned>(key: &str) -> Vec<T> {
    crate::local_storage()
        .and_then(|s| s.get_item(key).ok().flatten())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write_vec<T: serde::Serialize>(key: &str, v: &[T]) {
    if let Some(s) = crate::local_storage() {
        let _ = s.set_item(key, &serde_json::to_string(v).unwrap_or_default());
    }
}

/// Record a per-post event: OR the flag into the overlay (instant restyle),
/// queue it, and schedule a flush. Deduped: if the overlay already carries this
/// flag, nothing is new, so the queue is left untouched.
pub(crate) fn record_view(source: Source, post_id: &str, event: ViewEvent) {
    let key = (source.as_str().to_string(), post_id.to_string());
    let ov = overlay().0;
    let already = ov.with_untracked(|m| m.get(&key).copied().unwrap_or_default());
    let mut next = already;
    match event {
        ViewEvent::Seen => next.seen = true,
        ViewEvent::OpenedComments => {
            next.comments = true;
            next.seen = true;
        }
        ViewEvent::OpenedArticle => {
            next.article = true;
            next.seen = true;
        }
    }
    if next == already {
        return; // nothing new to record
    }
    ov.update(|m| {
        m.insert(key, next);
    });
    let mut q: Vec<ViewEventItem> = read_vec(OUTBOX_KEY);
    q.push(ViewEventItem {
        source,
        post_id: post_id.to_string(),
        event,
        at: now_utc(),
    });
    write_vec(OUTBOX_KEY, &q);
    schedule_flush();
}

/// Record a generic analytics event (`app_open`, `reader_open`, …).
pub(crate) fn record_event(kind: &str, detail: Option<String>) {
    let mut q: Vec<EventItem> = read_vec(EVENTS_KEY);
    q.push(EventItem {
        kind: kind.to_string(),
        detail,
        at: now_utc(),
    });
    write_vec(EVENTS_KEY, &q);
    schedule_flush();
}

/// Merge the server map into the overlay (OR only — never clears a
/// locally-pending flag that hasn't synced yet).
pub(crate) fn merge_server_map(source: Source, map: chaos_domain::ViewedMap) {
    let ov = overlay().0;
    ov.update(|m| {
        for (id, f) in map {
            let key = (source.as_str().to_string(), id);
            let cur = m.get(&key).copied().unwrap_or_default();
            m.insert(key, merge_flags(cur, f));
        }
    });
}

/// Log an `app_open` at most once per 5 min per device. Called at App boot once
/// a session exists.
pub(crate) fn maybe_record_app_open() {
    let now = (js_sys::Date::now() / 1000.0) as i64;
    let last = crate::local_storage()
        .and_then(|s| s.get_item(APPOPEN_KEY).ok().flatten())
        .and_then(|v| v.parse::<i64>().ok());
    if should_record_app_open(last, now) {
        if let Some(s) = crate::local_storage() {
            let _ = s.set_item(APPOPEN_KEY, &now.to_string());
        }
        record_event("app_open", None);
    }
}

/// The current-token client from the captured flush context, or `None` before
/// App boot (tests / components rendered outside App).
fn flush_client() -> Option<(ChaosClient, RwSignal<Connectivity>)> {
    FLUSH_CTX.with(|c| {
        c.borrow().as_ref().map(|(client, conn, persist)| {
            let token = persist.then(crate::stored_token).flatten();
            (client.clone().with_token(token), *conn)
        })
    })
}

/// Debounced (~1.5s trailing) flush: coalesce a burst of records into one POST.
/// Only POSTs when connectivity is Online; offline leaves the outboxes queued.
fn schedule_flush() {
    if FLUSH_SCHEDULED.get() {
        return;
    }
    let Some((client, conn)) = flush_client() else {
        return;
    };
    FLUSH_SCHEDULED.set(true);
    set_timeout(
        move || {
            FLUSH_SCHEDULED.set(false);
            if conn.get_untracked() == Connectivity::Online {
                let client = client.clone();
                spawn_local(async move { flush(client).await });
            }
        },
        std::time::Duration::from_millis(1500),
    );
}

/// Best-effort flush now (e.g. on the Offline→Online reconnect): if Online,
/// POST both outboxes. Grabs the current client/connectivity from context.
pub(crate) fn flush_now() {
    let Some((client, conn)) = flush_client() else {
        return;
    };
    if conn.get_untracked() == Connectivity::Online {
        spawn_local(async move { flush(client).await });
    }
}

/// POST both outboxes; clear each on its own success. A failed POST leaves that
/// outbox queued for the next flush.
pub(crate) async fn flush(client: ChaosClient) {
    let views: Vec<ViewEventItem> = read_vec(OUTBOX_KEY);
    if !views.is_empty()
        && client
            .record_views(&RecordViewsRequest {
                events: views.clone(),
            })
            .await
            .is_ok()
    {
        write_vec::<ViewEventItem>(OUTBOX_KEY, &[]);
    }
    let events: Vec<EventItem> = read_vec(EVENTS_KEY);
    if !events.is_empty()
        && client
            .record_events(&RecordEventsRequest {
                events: events.clone(),
            })
            .await
            .is_ok()
    {
        write_vec::<EventItem>(EVENTS_KEY, &[]);
    }
}

/// True if an `app_open` should be logged: never logged, or the last was
/// >= 5 min ago. `last_secs`/`now_secs` are unix seconds.
pub(crate) fn should_record_app_open(last_secs: Option<i64>, now_secs: i64) -> bool {
    match last_secs {
        None => true,
        Some(last) => now_secs - last >= 300,
    }
}

/// OR two flag sets together (each axis is monotonic: once true, stays true).
pub(crate) fn merge_flags(a: ViewFlags, b: ViewFlags) -> ViewFlags {
    ViewFlags {
        seen: a.seen || b.seen,
        comments: a.comments || b.comments,
        article: a.article || b.article,
    }
}

/// The dim class for a row. Dimming is the reading axis only; opening the
/// article suppresses the seen-dim (its check is the signal).
pub(crate) fn row_state_class(f: ViewFlags) -> &'static str {
    if f.comments {
        "read"
    } else if f.seen && !f.article {
        "seen"
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_open_throttle() {
        let now = 1_000_000i64; // seconds
        assert!(should_record_app_open(None, now)); // never opened
        assert!(should_record_app_open(Some(now - 301), now)); // > 5 min ago
        assert!(!should_record_app_open(Some(now - 299), now)); // < 5 min ago
    }

    #[test]
    fn merge_flags_is_or() {
        let a = ViewFlags {
            seen: true,
            comments: false,
            article: false,
        };
        let b = ViewFlags {
            seen: false,
            comments: true,
            article: false,
        };
        assert_eq!(
            merge_flags(a, b),
            ViewFlags {
                seen: true,
                comments: true,
                article: false
            }
        );
    }

    #[test]
    fn row_state_class_derivation() {
        let none = ViewFlags::default();
        let seen = ViewFlags { seen: true, ..none };
        let read = ViewFlags {
            seen: true,
            comments: true,
            ..none
        };
        let article = ViewFlags {
            seen: true,
            article: true,
            ..none
        };
        let both = ViewFlags {
            seen: true,
            comments: true,
            article: true,
        };
        assert_eq!(row_state_class(none), "");
        assert_eq!(row_state_class(seen), "seen");
        assert_eq!(row_state_class(read), "read");
        // article suppresses the seen-dim → no dim class (check still rendered separately)
        assert_eq!(row_state_class(article), "");
        assert_eq!(row_state_class(both), "read");
    }
}
