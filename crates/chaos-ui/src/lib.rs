//! Browser UI, mounted by the trunk web bundle. API configuration is
//! injected via [`AppConfig`].

mod analytics;
mod components;
mod date_util;
mod echarts;
mod hooks;
pub(crate) mod offline;
mod pages;
mod search;
mod weather_fetch;

use chaos_client::ChaosClient;
use chaos_domain::User;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::{A, Route, Router, Routes};
use leptos_router::path;
use url::Url;

/// Platform-provided configuration, put into the reactive context so every
/// component can reach the API client without prop-drilling.
#[derive(Clone)]
pub struct AppConfig {
    pub api_base: Url,
    /// The API is cross-origin (an explicit override): the
    /// HttpOnly session cookie won't flow there, so the session token from
    /// `login` is kept in localStorage and sent as a bearer header instead.
    pub persist_token: bool,
}

const TOKEN_KEY: &str = "chaos-token";
const API_BASE_KEY: &str = "chaos-api-base";
const THEME_KEY: &str = "chaos-theme";

/// Every selectable theme: (id, name, one-line description, swatches).
/// Pure CSS under `body[data-theme="…"]`; the first one is the default.
/// Chosen on the settings page, persisted per device.
pub const THEMES: &[Theme] = &[
    Theme {
        id: "campbell",
        name: "Campbell",
        description: "The Windows Terminal scheme: near-black, primary colors.",
        swatches: ["#0c0c0c", "#171717", "#3b78ff"],
    },
    Theme {
        id: "github",
        name: "GitHub Dark",
        description: "GitHub's dark mode: blue-tinted greys.",
        swatches: ["#0d1117", "#161b22", "#58a6ff"],
    },
    Theme {
        id: "midnight",
        name: "Midnight",
        description: "The original chaos look: slate blue-grey.",
        swatches: ["#14161c", "#1c1f27", "#7c9aff"],
    },
    Theme {
        id: "daylight",
        name: "Daylight",
        description: "Light mode for bright rooms.",
        swatches: ["#f3f4f8", "#ffffff", "#3b5bdb"],
    },
    Theme {
        id: "glass",
        name: "Glass",
        description: "Gradient backdrop, translucent cards, violet accent.",
        swatches: ["#101223", "#2a2440", "#b28dff"],
    },
    Theme {
        id: "terminal",
        name: "Terminal",
        description: "Flat monospace, green on black, dense.",
        swatches: ["#0b0e0b", "#10140f", "#6fdd8b"],
    },
];

#[derive(Clone, Copy)]
pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// Background / surface / accent preview dots.
    pub swatches: [&'static str; 3],
}

/// The active theme id, provided as context so the settings page can
/// change it from anywhere.
#[derive(Clone, Copy)]
pub struct ThemeSetting(pub RwSignal<String>);

pub fn use_theme() -> ThemeSetting {
    use_context::<ThemeSetting>().expect("ThemeSetting provided by App")
}

fn stored_theme() -> String {
    local_storage()
        .and_then(|s| s.get_item(THEME_KEY).ok().flatten())
        .filter(|v| THEMES.iter().any(|t| t.id == v))
        .unwrap_or_else(|| THEMES[0].id.to_string())
}

fn apply_theme(value: &str) {
    if let Some(body) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.body())
    {
        let _ = body.set_attribute("data-theme", value);
    }
    if let Some(storage) = local_storage() {
        let _ = storage.set_item(THEME_KEY, value);
    }
}

pub(crate) fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

fn stored_token() -> Option<String> {
    local_storage()?.get_item(TOKEN_KEY).ok()?
}

/// Persist (or clear, with `None`) the session token. Only called when
/// [`AppConfig::persist_token`] is set.
pub(crate) fn store_token(token: Option<&str>) {
    if let Some(storage) = local_storage() {
        let _ = match token {
            Some(token) => storage.set_item(TOKEN_KEY, token),
            None => storage.remove_item(TOKEN_KEY),
        };
    }
}

pub(crate) fn persist_token() -> bool {
    use_context::<AppConfig>().is_some_and(|config| config.persist_token)
}

// ---- device preferences (settings page) ----

pub(crate) const WEATHER_LOCATION_KEY: &str = "chaos-weather-location";
pub(crate) const WEATHER_UNITS_KEY: &str = "chaos-weather-units";
/// Locations compared on the weather page, newline-separated (names may
/// contain commas, e.g. "Osaka, JP").
pub(crate) const WEATHER_PLACES_KEY: &str = "chaos-weather-places";
/// Weather page view toggle: one combined chart vs one chart per place.
pub(crate) const WEATHER_COMBINED_KEY: &str = "chaos-weather-combined";

pub(crate) fn weather_places() -> Vec<String> {
    pref(WEATHER_PLACES_KEY)
        .map(|raw| {
            raw.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn set_weather_places(places: &[String]) {
    set_pref(WEATHER_PLACES_KEY, &places.join("\n"));
}

/// Whether the weather page shows the combined chart (true, the default)
/// or one chart per place.
pub(crate) fn weather_combined() -> bool {
    pref(WEATHER_COMBINED_KEY).as_deref() != Some("false")
}

pub(crate) fn set_weather_combined(combined: bool) {
    set_pref(
        WEATHER_COMBINED_KEY,
        if combined { "true" } else { "false" },
    );
}

// ---- news page preferences (device, persisted) ----

/// Which posts provider the news page opens on (HN vs lobste.rs).
pub(crate) const NEWS_SOURCE_KEY: &str = "chaos-news-source";
/// Which trailing window the news page opens on: "0"=24h, "1"=48h, "2"=week.
pub(crate) const NEWS_RANGE_KEY: &str = "chaos-news-range";

/// Pure mapping of a stored source string to a [`Source`]; unknown/absent
/// falls back to the HackerNews default.
fn news_source_from(raw: Option<&str>) -> chaos_domain::Source {
    raw.and_then(chaos_domain::Source::from_str)
        .unwrap_or(chaos_domain::Source::HackerNews)
}

pub(crate) fn news_source() -> chaos_domain::Source {
    news_source_from(pref(NEWS_SOURCE_KEY).as_deref())
}

pub(crate) fn set_news_source(source: chaos_domain::Source) {
    set_pref(NEWS_SOURCE_KEY, source.as_str());
}

/// The news range index: 0=24h, 1=48h, 2=week (default 0).
pub(crate) fn news_range() -> u8 {
    pref(NEWS_RANGE_KEY)
        .and_then(|v| v.parse().ok())
        .filter(|n| *n <= 2)
        .unwrap_or(0)
}

pub(crate) fn set_news_range(idx: u8) {
    set_pref(NEWS_RANGE_KEY, &idx.to_string());
}

/// Celsius in the display unit: °F when the preference says so.
pub(crate) fn convert_temp(celsius: f64, fahrenheit: bool) -> f64 {
    if fahrenheit {
        celsius * 9.0 / 5.0 + 32.0
    } else {
        celsius
    }
}

/// Converted temperature rounded to one decimal — chart series values that
/// land verbatim in tooltips.
pub(crate) fn convert_temp_1dp(celsius: f64, fahrenheit: bool) -> f64 {
    (convert_temp(celsius, fahrenheit) * 10.0).round() / 10.0
}

/// Displayed temperature honoring the °C/°F preference; the wire is metric.
pub(crate) fn format_temp(celsius: f64, fahrenheit: bool) -> String {
    format!("{:.0}°", convert_temp(celsius, fahrenheit))
}

/// The "lead · feels X° · wind Y km/h · Z% humidity" line shared by the
/// dashboard weather widget (lead = location) and the weather page rows
/// (lead = description).
pub(crate) fn weather_details(
    lead: &str,
    weather: &chaos_domain::WeatherData,
    fahrenheit: bool,
) -> String {
    format!(
        "{} · feels {} · wind {:.0} km/h{}",
        lead,
        format_temp(weather.apparent_c, fahrenheit),
        weather.wind_kmh,
        weather
            .humidity_pct
            .map(|h| format!(" · {h:.0}% humidity"))
            .unwrap_or_default(),
    )
}

pub(crate) fn weather_emoji(code: i32) -> &'static str {
    match code {
        0 => "☀️",
        1 => "🌤️",
        2 => "⛅",
        3 => "☁️",
        45 | 48 => "🌫️",
        51..=57 => "🌦️",
        61..=67 | 80..=82 => "🌧️",
        71..=77 | 85 | 86 => "🌨️",
        95..=99 => "⛈️",
        _ => "🌡️",
    }
}

thread_local! {
    /// Units default reported by the server's health check (its host locale,
    /// LC_MEASUREMENT and friends). Set by the gate before pages render.
    static SERVER_FAHRENHEIT: std::cell::Cell<Option<bool>> =
        const { std::cell::Cell::new(None) };
}

pub(crate) fn set_server_fahrenheit(value: Option<bool>) {
    SERVER_FAHRENHEIT.set(value);
}

/// °F or °C for displayed temperatures: the device preference when one is
/// set, otherwise the system default.
pub(crate) fn weather_fahrenheit() -> bool {
    match pref(WEATHER_UNITS_KEY).as_deref() {
        Some("fahrenheit") => true,
        Some(_) => false,
        None => default_fahrenheit(),
    }
}

/// The "system locale" units default: what the server host's locale says
/// (it sees LC_MEASUREMENT; browsers don't), else the browser language
/// region as a rough proxy.
pub(crate) fn default_fahrenheit() -> bool {
    SERVER_FAHRENHEIT.get().unwrap_or_else(fahrenheit_locale)
}

/// True when the browser language's region customarily uses Fahrenheit —
/// only a proxy: `navigator.language` is the browser UI language, not the
/// OS locale.
fn fahrenheit_locale() -> bool {
    // The short list of countries still on °F (US and its close orbit).
    const FAHRENHEIT_REGIONS: [&str; 8] = ["US", "BS", "BZ", "KY", "LR", "PW", "FM", "MH"];
    web_sys::window()
        .and_then(|w| w.navigator().language())
        .is_some_and(|lang| {
            lang.split('-').any(|part| {
                part.len() == 2
                    && part.chars().all(|c| c.is_ascii_uppercase())
                    && FAHRENHEIT_REGIONS.contains(&part)
            })
        })
}

/// A device preference; `None`/empty means "server default".
pub(crate) fn pref(key: &str) -> Option<String> {
    local_storage()?
        .get_item(key)
        .ok()?
        .filter(|v| !v.trim().is_empty())
}

pub(crate) fn set_pref(key: &str, value: &str) {
    if let Some(storage) = local_storage() {
        let _ = if value.trim().is_empty() {
            storage.remove_item(key)
        } else {
            storage.set_item(key, value.trim())
        };
    }
}

/// The per-device server override (Settings). `None`
/// means the page origin — see chaos-web's `resolve()`.
pub(crate) fn api_base_override() -> Option<String> {
    local_storage()?.get_item(API_BASE_KEY).ok()?
}

/// Persist (or clear) the server override, then reload so the whole app —
/// client, session, gate — starts over against the new server.
pub(crate) fn set_api_base_override(value: Option<&str>) {
    if let Some(storage) = local_storage() {
        let _ = match value {
            Some(value) => storage.set_item(API_BASE_KEY, value),
            None => storage.remove_item(API_BASE_KEY),
        };
    }
    if let Some(window) = web_sys::window() {
        let _ = window.location().reload();
    }
}

/// Who is signed in, if anyone. `None` is a first-class state: everything
/// except calendars works logged off.
#[derive(Clone, Copy)]
pub struct Session(pub RwSignal<Option<User>>);

/// The News tab's loaded payload and scroll offset, held here rather than in
/// the page so it survives navigation: the router drops and rebuilds
/// `NewsPage` on every visit, which is what made going back re-fetch from cold
/// and flash a loading state. Refreshing is then an explicit act — the
/// pull-to-refresh gesture, or a page reload in a desktop browser.
///
/// In memory only: a cold start still fetches.
#[derive(Clone, Copy, Default)]
pub struct NewsCache {
    /// `(source, posts, has_more)` exactly as `load_posts` returned it.
    #[allow(clippy::type_complexity)]
    pub loaded: RwSignal<Option<(chaos_domain::Source, chaos_domain::PostsData, bool)>>,
    /// Vertical scroll offset to restore, in px.
    pub scroll: RwSignal<f64>,
}

pub(crate) fn use_news_cache() -> NewsCache {
    use_context::<NewsCache>().expect("NewsCache provided by App")
}

/// One HTTP client for the whole app, provided as context at `App`.
/// `use_client()` clones it per call (reqwest clients are `Arc`s inside),
/// so components share the connection pool instead of building a new
/// `reqwest::Client` on every call.
#[derive(Clone)]
struct SharedClient(ChaosClient);

/// The current session token for detached tasks that cannot read context.
pub(crate) fn current_token() -> Option<String> {
    // No `persist_token()` here: it reads reactive context, and this is called
    // from detached tasks where there is none, so the fallback would silently
    // vanish. Reading the stored token unconditionally is equivalent — login
    // only writes it when `persist_token()` was true in the first place.
    stored_token()
}

pub fn use_client() -> ChaosClient {
    let config = use_context::<AppConfig>().expect("AppConfig provided by App");
    // The token is read per call, not per app: it changes on login/logout
    // and callers must always see the current one (matches the previous
    // behavior, where the whole client was rebuilt per call).
    let token = config.persist_token.then(stored_token).flatten();
    match use_context::<SharedClient>() {
        Some(SharedClient(client)) => client.with_token(token),
        // Components rendered outside App (tests) fall back to a
        // one-off client. Logged so a refactor that loses the context shows
        // up instead of silently rebuilding a client per call.
        None => {
            leptos::logging::debug_warn!("use_client: no SharedClient in context");
            ChaosClient::new(config.api_base).with_token(token)
        }
    }
}

pub fn use_session() -> Session {
    use_context::<Session>().expect("Session provided by App")
}

/// Sign-out shared by the topbar and the More page: server-side logout,
/// stored token cleared, session signal reset.
pub(crate) fn use_logout() -> Callback<leptos::ev::MouseEvent> {
    let session = use_session();
    let client = use_client();
    Callback::new(move |_: leptos::ev::MouseEvent| {
        let client = client.clone();
        spawn_local(async move {
            let _ = client.logout().await;
            store_token(None);
            offline::cache_clear();
            session.0.set(None);
        });
    })
}

/// Primary navigation destinations. The glyphs are plain Unicode (like
/// yomu's tab bar) so no icon font or SVG set is needed.
const NAV_PRIMARY: [(&str, &str, &str); 5] = [
    ("/", "▦", "Dash"),
    ("/links", "⛓", "Links"),
    ("/news", "▤", "News"),
    ("/weather", "☀", "Weather"),
    ("/more", "≡", "More"),
];

#[component]
pub fn App(config: AppConfig) -> impl IntoView {
    // Clear credentials left by retired native-app authentication.
    for stale in [
        "chaos-authentik-user",
        "chaos-authentik-token",
        "chaos-oidc-access",
    ] {
        set_pref(stale, "");
    }

    let api_base = config.api_base.clone();
    provide_context(config);
    provide_context(SharedClient(ChaosClient::new(api_base)));

    // Offline support: one app-wide connectivity signal, Checking until the
    // gate's first probe answers. The browser's `online` event buys one free
    // re-probe (it only says "some network came back", not "the server is
    // reachable", so it triggers a probe rather than trusting it).
    let conn = RwSignal::new(offline::Connectivity::Checking);
    provide_context(conn);

    let client_for_online = use_client();
    let online_probe = window_event_listener(leptos::ev::online, move |_| {
        let client = client_for_online.clone();
        spawn_local(async move {
            offline::probe(&client, conn).await;
        });
    });
    on_cleanup(move || online_probe.remove());

    let session = Session(RwSignal::new(None));
    provide_context(session);
    // Outlives route changes on purpose — see NewsCache.
    provide_context(NewsCache::default());

    // Analytics overlay + flush context (needs AppConfig, SharedClient and the
    // connectivity signal, all provided above).
    analytics::provide_overlay();

    // Restore the session. A one-shot `me()` on mount would leave the
    // session empty forever on an offline boot (locking per-user pages like
    // the calendar behind a sign-in prompt despite cached data), so instead:
    // on the first run serve the cached last-known user immediately —
    // harmless, since offline there are no API calls to authorize — and on
    // every Offline/Checking → Online transition (including the gate's
    // first successful probe) re-validate with a real `me()` call.
    let client = use_client();
    let was_online = StoredValue::new(false);
    Effect::new(move |prev: Option<()>| {
        let now_online = conn.get() == offline::Connectivity::Online;
        let came_online = now_online && !was_online.get_value();
        was_online.set_value(now_online);
        if prev.is_none()
            && !now_online
            && let Some(user) = offline::cache_get::<User>("me")
        {
            session.0.set(Some(user));
            analytics::maybe_record_app_open();
        }
        if !came_online {
            return;
        }
        let client = client.clone();
        spawn_local(async move {
            let client = client.with_token(current_token());
            match client.me().await {
                Ok(user) => {
                    // Cache the signed-in user so the next offline boot can
                    // restore the session; overwritten on every success and
                    // dropped by logout's cache_clear.
                    offline::cache_put("me", &user);
                    session.0.set(Some(user));
                    analytics::maybe_record_app_open();
                }
                // Lost the server between the probe and this call: keep (or
                // restore) the last-known user; the next Online transition
                // re-validates.
                Err(chaos_client::ClientError::Transport(_)) => {
                    if let Some(user) = offline::cache_get::<User>("me") {
                        session.0.set(Some(user));
                    }
                }
                // An expired or revoked session must not survive in the cache.
                Err(chaos_client::ClientError::Api { status: 401, .. }) => {
                    offline::cache_remove("me");
                    session.0.set(None);
                }
                // Any other API error says nothing about the session.
                Err(chaos_client::ClientError::Api { .. }) => {}
                // Decode noise says nothing about the session; change nothing.
                Err(_) => {}
            }
        });
    });

    // Theme: applied as `data-theme` on <body>, persisted, all-CSS.
    // Changed from the settings page via the ThemeSetting context.
    let theme = ThemeSetting(RwSignal::new(stored_theme()));
    provide_context(theme);
    Effect::new(move |_| apply_theme(&theme.0.get()));

    // Global quick-search: Ctrl-K (Cmd-K on mac) toggles the overlay from
    // anywhere. Window-level listener, removed on unmount.
    let search_open = RwSignal::new(false);
    let search_keys = window_event_listener(leptos::ev::keydown, move |ev| {
        if (ev.ctrl_key() || ev.meta_key()) && ev.key().eq_ignore_ascii_case("k") {
            ev.prevent_default();
            search_open.update(|o| *o = !*o);
        }
    });
    on_cleanup(move || search_keys.remove());

    let logout = use_logout();

    view! {
        <ServerGate>
        <Router>
            <search::QuickSearch open=search_open/>
            <offline::OfflineBadge/>
            <nav class="topbar">
                <span class="brand">"chaos"</span>
                <A href="/"><span class="nav-icon">"▦"</span>"Dashboard"</A>
                <A href="/links"><span class="nav-icon">"⛓"</span>"Links"</A>
                <A href="/news"><span class="nav-icon">"▤"</span>"News"</A>
                <A href="/calendar"><span class="nav-icon">"▣"</span>"Calendar"</A>
                <A href="/weather"><span class="nav-icon">"☀"</span>"Weather"</A>
                <A href="/home"><span class="nav-icon">"⌂"</span>"Home"</A>
                <button
                    class="topbar-search"
                    title="Search (Ctrl-K)"
                    on:click=move |_| search_open.set(true)
                >
                    <span class="nav-icon">"⌕"</span>
                    "Search"
                    <kbd>"Ctrl K"</kbd>
                </button>
                <span class="topbar-foot">
                    <A href="/settings"><span class="nav-icon">"⚙"</span>"Settings"</A>
                    <A href="/about"><span class="nav-icon">"ⓘ"</span>"About"</A>
                    <span class="topbar-account">
                        // Behind authentik, "Sign out" clears the local chaos
                        // token but the greeting persists: `me()` still resolves
                        // via the forwarded proxy header (the authentik session
                        // is separate). That's expected.
                        {move || match session.0.get() {
                            Some(user) => {
                                view! {
                                    <span class="topbar-user">"Hello " {user.display_name}</span>
                                    <button
                                        class="topbar-logout"
                                        title="Sign out"
                                        on:click=move |ev| logout.run(ev)
                                    >
                                        "Sign out"
                                    </button>
                                }
                                    .into_any()
                            }
                            None => {
                                view! {
                                    <span class="topbar-user topbar-stranger">"Hello stranger"</span>
                                    <A href="/login">"Sign in"</A>
                                }
                                    .into_any()
                            }
                        }}
                    </span>
                </span>
            </nav>
            <nav class="tabbar">
                {NAV_PRIMARY
                    .into_iter()
                    .map(|(href, icon, label)| view! {
                        <A href=href>
                            <span class="tab-icon">{icon}</span>
                            <span class="tab-label">{label}</span>
                        </A>
                    })
                    .collect_view()}
                <button class="tab-search" on:click=move |_| search_open.set(true)>
                    <span class="tab-icon">"⌕"</span>
                    <span class="tab-label">"Search"</span>
                </button>
            </nav>
            <main>
                <Routes fallback=|| view! { <p class="muted">"Page not found"</p> }>
                    <Route path=path!("/") view=pages::Dashboard/>
                    <Route path=path!("/links") view=pages::Links/>
                    <Route path=path!("/news") view=pages::NewsPage/>
                    // The static `/news` route above wins over this param route
                    // in leptos_router, so the list page is never shadowed.
                    <Route path=path!("/news/:source/:id") view=pages::PostReader/>
                    <Route path=path!("/calendar") view=pages::CalendarPage/>
                    <Route path=path!("/weather") view=pages::WeatherPage/>
                    <Route path=path!("/home") view=pages::HomePage/>
                    <Route path=path!("/login") view=pages::Login/>
                    <Route path=path!("/settings") view=pages::Settings/>
                    <Route path=path!("/more") view=pages::MorePage/>
                    <Route path=path!("/about") view=pages::AboutPage/>
                </Routes>
            </main>
        </Router>
        </ServerGate>
    }
}

/// Probe before mounting resources; known offline servers can use cached data.
#[component]
fn ServerGate(children: ChildrenFn) -> impl IntoView {
    let ready = RwSignal::new(false);
    let client = use_client();
    let conn = offline::use_connectivity();
    spawn_local(async move {
        if offline::probe(&client, conn).await.is_none() {
            set_server_fahrenheit(
                offline::cache_get::<Option<bool>>("server-fahrenheit").flatten(),
            );
        }
        ready.set(true);
    });

    view! {
        {move || if ready.get() {
            children().into_any()
        } else {
            view! { <p class="muted gate-msg">"Connecting…"</p> }.into_any()
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_weather() -> chaos_domain::WeatherData {
        chaos_domain::WeatherData {
            location: "Paris, FR".into(),
            temperature_c: 21.0,
            apparent_c: 19.6,
            humidity_pct: Some(55.0),
            wind_kmh: 12.3,
            weather_code: 1,
            description: "Mainly clear".into(),
            daily: Vec::new(),
            hourly: Vec::new(),
            now_index: 0,
        }
    }

    #[test]
    fn news_source_parses() {
        assert_eq!(
            news_source_from(Some("lobsters")),
            chaos_domain::Source::Lobsters
        );
        assert_eq!(news_source_from(None), chaos_domain::Source::HackerNews);
        assert_eq!(
            news_source_from(Some("garbage")),
            chaos_domain::Source::HackerNews
        );
    }

    #[test]
    fn convert_temp_handles_both_units() {
        assert_eq!(convert_temp(0.0, false), 0.0);
        assert_eq!(convert_temp(0.0, true), 32.0);
        assert_eq!(convert_temp(100.0, true), 212.0);
        assert_eq!(convert_temp_1dp(21.34, false), 21.3);
        assert_eq!(convert_temp_1dp(21.34, true), 70.4);
        assert_eq!(format_temp(19.6, false), "20°");
        assert_eq!(format_temp(19.6, true), "67°");
    }

    #[test]
    fn weather_details_joins_the_parts() {
        let weather = sample_weather();
        assert_eq!(
            weather_details("Paris, FR", &weather, false),
            "Paris, FR · feels 20° · wind 12 km/h · 55% humidity"
        );
        let mut weather = weather;
        weather.humidity_pct = None;
        assert_eq!(
            weather_details("Mainly clear", &weather, true),
            "Mainly clear · feels 67° · wind 12 km/h"
        );
    }
}
