//! Viewed-state + engagement analytics: the offline-capable outbox, the
//! optimistic overlay, the debounced/reconnect flush, and the `app_open`
//! throttle. The PURE helpers (throttle predicate, flag merge, row class) are
//! unit-tested here; the localStorage/timer/observer glue is browser-only.

// Consumers land across B2-B6 (outbox glue, App boot, rows, reader). Until the
// last of those is wired, some items are defined ahead of their first use.
#![allow(dead_code)]

use chaos_domain::ViewFlags;

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
