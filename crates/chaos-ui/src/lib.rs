//! Shared Leptos UI: the same `App` component is mounted by the web bundle
//! (trunk) and rendered inside the Tauri webview. Anything platform-specific
//! (like where the API lives) is injected from the outside via [`AppConfig`].

mod analytics;
mod components;
mod date_util;
mod echarts;
mod hooks;
pub(crate) mod offline;
mod pages;
mod search;
mod tauri_http;
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
    /// The API is cross-origin (Tauri shell or explicit override): the
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

/// The per-device server override (settings page / connect screen). `None`
/// means the platform default: the page origin on web, the bundled default
/// in the shells — see chaos-web's `resolve()`.
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

/// One HTTP client for the whole app, provided as context at `App`.
/// `use_client()` clones it per call (reqwest clients are `Arc`s inside),
/// so components share the connection pool instead of building a new
/// `reqwest::Client` on every call.
#[derive(Clone)]
struct SharedClient(ChaosClient);

/// Ask the Android shell to launch a companion app natively. True only when
/// the app is installed and claimed the tap; false (not installed, or no
/// bridge) means the caller should let the URL open normally.
pub(crate) fn open_app_native(package: &str) -> bool {
    use leptos::wasm_bindgen::JsCast;
    let Some(window) = web_sys::window() else {
        return false;
    };
    if let Ok(bridge) = js_sys::Reflect::get(&window, &"ChaosAndroid".into())
        && !bridge.is_undefined()
        && let Ok(f) = js_sys::Reflect::get(&bridge, &"openApp".into())
        && let Ok(f) = f.dyn_into::<js_sys::Function>()
    {
        return f
            .call1(&bridge, &package.into())
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
    }
    false
}

/// Open a URL outside the webview. In the Android shell that is a VIEW
/// intent (an installed app that registered the URL claims it, the default
/// browser otherwise); in the desktop shell it is the `open_external`
/// command (xdg-open and friends). Returns false in a plain browser, where
/// the anchor's own `target="_blank"` is already the right behavior.
pub(crate) fn open_external(url: &str) -> bool {
    use leptos::wasm_bindgen::JsCast;
    let Some(window) = web_sys::window() else {
        return false;
    };
    if let Ok(bridge) = js_sys::Reflect::get(&window, &"ChaosAndroid".into())
        && !bridge.is_undefined()
        && let Ok(f) = js_sys::Reflect::get(&bridge, &"openUrl".into())
        && let Ok(f) = f.dyn_into::<js_sys::Function>()
    {
        let _ = f.call1(&bridge, &url.into());
        return true;
    }
    if let Ok(tauri) = js_sys::Reflect::get(&window, &"__TAURI__".into())
        && !tauri.is_undefined()
        && let Ok(core) = js_sys::Reflect::get(&tauri, &"core".into())
        && let Ok(invoke) = js_sys::Reflect::get(&core, &"invoke".into())
        && let Ok(invoke) = invoke.dyn_into::<js_sys::Function>()
    {
        let args = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&args, &"url".into(), &url.into());
        let _ = invoke.call2(&core, &"open_external".into(), &args);
        return true;
    }
    false
}

/// True inside the Android shell (it injects `window.CHAOS_PLATFORM`).
pub(crate) fn on_android() -> bool {
    web_sys::window()
        .and_then(|w| js_sys::Reflect::get(&w, &"CHAOS_PLATFORM".into()).ok())
        .and_then(|v| v.as_string())
        .is_some_and(|p| p == "android")
}

pub fn use_client() -> ChaosClient {
    let config = use_context::<AppConfig>().expect("AppConfig provided by the shell");
    // The token is read per call, not per app: it changes on login/logout
    // and callers must always see the current one (matches the previous
    // behavior, where the whole client was rebuilt per call).
    let token = config.persist_token.then(stored_token).flatten();
    match use_context::<SharedClient>() {
        Some(SharedClient(client)) => client.with_token(token),
        // Components rendered outside App (tests, shells) fall back to a
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
                // The server answered "no session" (expired/revoked): the
                // cached user is stale — drop it and stay signed out.
                Err(chaos_client::ClientError::Api { .. }) => {
                    offline::cache_remove("me");
                    session.0.set(None);
                }
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
    // anywhere. Window-level listener, removed on unmount like the click
    // interceptor below.
    let search_open = RwSignal::new(false);
    let search_keys = window_event_listener(leptos::ev::keydown, move |ev| {
        if (ev.ctrl_key() || ev.meta_key()) && ev.key().eq_ignore_ascii_case("k") {
            ev.prevent_default();
            search_open.update(|o| *o = !*o);
        }
    });
    on_cleanup(move || search_keys.remove());

    // Inside a shell, clicking an outbound link must not navigate the
    // webview: one document-level interceptor reroutes every external
    // http(s) anchor through the system opener (covers all target="_blank"
    // links at once). Same-origin anchors — the SPA's own routes — pass
    // through untouched, as does everything in a plain browser.
    let external_clicks = window_event_listener(leptos::ev::click, |ev| {
        use leptos::wasm_bindgen::JsCast;
        let Some(target) = ev.target() else { return };
        let Ok(el) = target.dyn_into::<web_sys::Element>() else {
            return;
        };
        let Ok(Some(anchor)) = el.closest("a[href]") else {
            return;
        };
        let Ok(anchor) = anchor.dyn_into::<web_sys::HtmlAnchorElement>() else {
            return;
        };
        let href = anchor.href();
        let origin = window().location().origin().unwrap_or_default();
        let external = (href.starts_with("http://") || href.starts_with("https://"))
            && !href.starts_with(&origin);
        if external && !ev.default_prevented() && open_external(&href) {
            ev.prevent_default();
        }
    });
    on_cleanup(move || external_clicks.remove());

    let logout = use_logout();

    view! {
        <ServerGate>
        <Router>
            <ShareRedirect/>
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
                        {move || match session.0.get() {
                            Some(user) => {
                                view! {
                                    <span class="topbar-user">{user.display_name}</span>
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
                            None => view! { <A href="/login">"Sign in"</A> }.into_any(),
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

/// Android share-sheet entry: the shell cold-loads `/?share=<text>` (only
/// the root path is guaranteed to resolve in Tauri's asset protocol), and
/// this forwards the payload to the links quick-add.
#[component]
fn ShareRedirect() -> impl IntoView {
    let navigate = leptos_router::hooks::use_navigate();
    let shared = web_sys::window()
        .and_then(|w| w.location().search().ok())
        .and_then(|search| {
            url::form_urlencoded::parse(search.trim_start_matches('?').as_bytes())
                .find(|(k, _)| k == "share")
                .map(|(_, v)| v.into_owned())
        });
    if let Some(text) = shared {
        let target = format!(
            "/links?add={}",
            url::form_urlencoded::byte_serialize(text.as_bytes()).collect::<String>()
        );
        // Deferred: navigating during the initial render is a no-op.
        leptos::task::spawn_local(async move {
            navigate(
                &target,
                leptos_router::NavigateOptions {
                    replace: true,
                    ..Default::default()
                },
            );
        });
    }
}

#[derive(Clone, Copy, PartialEq)]
enum GateState {
    Checking,
    Ready,
    Unreachable,
}

/// Blocks the app behind a health check so a shell pointing at the wrong
/// place gets a "connect to your server" form instead of a wall of failed
/// requests. The chosen URL is the `chaos-api-base` localStorage override
/// that the web entrypoint's API-base resolution already honors.
#[component]
fn ServerGate(children: ChildrenFn) -> impl IntoView {
    let gate = RwSignal::new(GateState::Checking);
    let client = use_client();
    let conn = offline::use_connectivity();
    let seen = offline::server_seen(use_client().base().as_str());
    spawn_local(async move {
        // `probe` handles set_server_fahrenheit and mark_server_seen. A
        // server we've reached before is just offline right now: boot into
        // the cached UI with the badge instead of the connect form.
        if offline::probe(&client, conn).await {
            gate.set(GateState::Ready);
        } else if seen {
            // Known server, just offline right now: the probe couldn't
            // deliver the server's °F/°C default, so restore the one it
            // reported last time. `cache_get` wraps the stored value in its
            // own Option (Option<Option<bool>>: outer = "was it cached",
            // inner = the server's own answer); flatten collapses "never
            // cached" and "server said None" — both mean locale fallback.
            set_server_fahrenheit(
                offline::cache_get::<Option<bool>>("server-fahrenheit").flatten(),
            );
            gate.set(GateState::Ready);
        } else {
            gate.set(GateState::Unreachable);
        }
    });

    let current = use_client().base().to_string();
    let input = RwSignal::new(current);
    let connect = move |_| {
        let value = input.get_untracked().trim().to_string();
        if Url::parse(&value).is_err() {
            return;
        }
        set_api_base_override(Some(&value));
    };

    view! {
        {move || match gate.get() {
            GateState::Checking => view! { <p class="muted gate-msg">"Connecting…"</p> }.into_any(),
            GateState::Ready => children().into_any(),
            GateState::Unreachable => {
                view! {
                    <section class="server-gate">
                        <h2>"Cannot reach the chaos server"</h2>
                        <p class="muted">
                            "Enter the address of your server (for example "
                            <code>"http://zeus:4600"</code> ")."
                        </p>
                        <div class="gate-form">
                            <input
                                type="url"
                                prop:value=move || input.get()
                                on:input=move |ev| input.set(event_target_value(&ev))
                            />
                            <button class="primary" on:click=connect>
                                "Connect"
                            </button>
                            <button on:click=move |_| gate.set(GateState::Ready)>
                                "Continue anyway"
                            </button>
                        </div>
                    </section>
                }
                    .into_any()
            }
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
