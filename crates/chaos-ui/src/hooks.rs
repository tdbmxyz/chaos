//! Reusable reactive scaffolding for the dashboard widgets: interval-driven
//! polling and busy/error bookkeeping around fire-and-forget actions.

use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;

/// Bumped by the dashboard's manual refresh button; every polled resource
/// tracks it when it is in context.
#[derive(Clone, Copy)]
pub(crate) struct RefreshTick(pub(crate) RwSignal<u32>);

/// A counter signal bumped every `interval` for as long as the current
/// reactive owner lives.
pub(crate) fn use_interval_tick(interval: Duration) -> RwSignal<u32> {
    let tick = RwSignal::new(0u32);
    if let Ok(handle) = set_interval_with_handle(move || tick.update(|n| *n += 1), interval) {
        on_cleanup(move || handle.clear());
    }
    tick
}

/// A read-only signal that follows `source` once it has been stable for
/// `delay` (a trailing debounce): typing in the search box only queries the
/// server after the user pauses.
pub(crate) fn debounce_signal(source: RwSignal<String>, delay: Duration) -> Signal<String> {
    let out = RwSignal::new(source.get_untracked());
    let generation = StoredValue::new(0u64);
    // Like use_interval_tick, the pending timer is cleared on owner
    // disposal (and superseded runs are also fenced by `generation`).
    let pending = StoredValue::new(None::<TimeoutHandle>);
    Effect::new(move |_| {
        let value = source.get();
        let current = generation.with_value(|g| *g + 1);
        generation.set_value(current);
        let handle = set_timeout_with_handle(
            move || {
                if generation.get_value() == current {
                    out.set(value);
                }
            },
            delay,
        )
        .ok();
        if let Some(previous) = pending.with_value(|p| *p) {
            previous.clear();
        }
        pending.set_value(handle);
    });
    on_cleanup(move || {
        if let Some(handle) = pending.with_value(|p| *p) {
            handle.clear();
        }
    });
    out.into()
}

/// A [`LocalResource`] re-run every `interval`, whenever the dashboard-wide
/// [`RefreshTick`] bumps, and whenever `version` (an action's success
/// counter, see [`use_action`]) changes. Pass `None` for resources without
/// a mutating action.
///
/// Thin wrapper over [`use_polled_resource_with`] with `poll_offline: false`
/// — the common case: pause polling while the chaos server is unreachable.
pub(crate) fn use_polled_resource<T, Fut>(
    interval: Duration,
    version: Option<RwSignal<u32>>,
    fetch: impl Fn() -> Fut + 'static,
) -> LocalResource<T>
where
    T: 'static,
    Fut: Future<Output = T> + 'static,
{
    use_polled_resource_with(interval, version, false, fetch)
}

/// Like [`use_polled_resource`], but with an offline escape hatch.
///
/// While offline, interval ticks and manual refreshes must not fire
/// requests at an unreachable server: with `poll_offline: false` those
/// sources go untracked, so the resource simply stops re-running (`version`
/// is still always tracked). Recovery still works — the connectivity signal
/// itself is read unconditionally below, which re-runs the resource once
/// connectivity flips back to Online. The same tracked read also re-runs the
/// resource once on a connectivity *downgrade*, so fetches must go through
/// [`crate::offline::cached`] (which serves the cache without touching the
/// network while offline) for the no-probing guarantee to hold.
///
/// `poll_offline: true` keeps polling even while offline; it's the escape
/// hatch for widgets that fetch their data without going through the chaos
/// server at all (e.g. weather/HN direct-fetch widgets, wired up in a later
/// plan) — those have no reason to pause just because the chaos server is
/// unreachable.
///
/// Components rendered outside `App` (unit tests) have no `Connectivity`
/// context; treat that as Online rather than panicking.
#[allow(dead_code, reason = "used by the direct-fetch widgets plan")]
pub(crate) fn use_polled_resource_with<T, Fut>(
    interval: Duration,
    version: Option<RwSignal<u32>>,
    poll_offline: bool,
    fetch: impl Fn() -> Fut + 'static,
) -> LocalResource<T>
where
    T: 'static,
    Fut: Future<Output = T> + 'static,
{
    let tick = use_interval_tick(interval);
    let refresh = use_context::<RefreshTick>();
    let conn = use_context::<RwSignal<crate::offline::Connectivity>>();
    LocalResource::new(move || {
        // Tracked read: this is what makes recovery re-run the resource
        // once connectivity flips back to Online.
        let online = conn
            .map(|c| c.get() == crate::offline::Connectivity::Online)
            .unwrap_or(true);
        if poll_offline || online {
            tick.track();
            if let Some(RefreshTick(refresh)) = refresh {
                refresh.track();
            }
        }
        if let Some(version) = version {
            version.track();
        }
        fetch()
    })
}

/// Signals around an async action: `busy` while it runs, `error` carrying
/// the last failure, `version` bumped on success so polled resources
/// refetch right away instead of on the next poll.
#[derive(Clone, Copy)]
pub(crate) struct ActionState {
    pub version: RwSignal<u32>,
    pub busy: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
}

/// Wrap an async operation in busy/error bookkeeping; returns the state
/// plus the [`Callback`] to hand to buttons.
pub(crate) fn use_action<I, Fut, T, E>(
    run: impl Fn(I) -> Fut + Send + Sync + 'static,
) -> (ActionState, Callback<I>)
where
    I: Send + 'static,
    Fut: Future<Output = Result<T, E>> + 'static,
    T: 'static,
    E: std::fmt::Display + 'static,
{
    let state = ActionState {
        version: RwSignal::new(0u32),
        busy: RwSignal::new(false),
        error: RwSignal::new(None),
    };
    let callback = Callback::new(move |input: I| {
        let fut = run(input);
        state.busy.set(true);
        state.error.set(None);
        spawn_local(async move {
            match fut.await {
                Ok(_) => state.version.update(|n| *n += 1),
                Err(err) => state.error.set(Some(err.to_string())),
            }
            state.busy.set(false);
        });
    });
    (state, callback)
}
