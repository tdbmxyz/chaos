//! Shared Leptos UI: the same `App` component is mounted by the web bundle
//! (trunk) and rendered inside the Tauri webview. Anything platform-specific
//! (like where the API lives) is injected from the outside via [`AppConfig`].

mod components;
mod pages;

use chaos_client::ChaosClient;
use chaos_domain::{AppLink, User};
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

fn local_storage() -> Option<web_sys::Storage> {
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

/// Who is signed in, if anyone. `None` is a first-class state: everything
/// except calendars works logged off.
#[derive(Clone, Copy)]
pub struct Session(pub RwSignal<Option<User>>);

/// Companion apps activated on the server (empty when the feature is off).
#[derive(Clone, Copy)]
pub struct Apps(pub RwSignal<Vec<AppLink>>);

pub fn use_apps() -> Apps {
    use_context::<Apps>().expect("Apps provided by App")
}

/// Try the Android shell's native bridge (launches the app when installed,
/// otherwise views the URL); falls back to a plain window.open elsewhere.
fn open_app_native(package: &str, url: &str) {
    use leptos::wasm_bindgen::JsCast;
    let Some(window) = web_sys::window() else {
        return;
    };
    if let Ok(bridge) = js_sys::Reflect::get(&window, &"ChaosAndroid".into())
        && !bridge.is_undefined()
        && let Ok(f) = js_sys::Reflect::get(&bridge, &"openApp".into())
        && let Ok(f) = f.dyn_into::<js_sys::Function>()
    {
        let _ = f.call2(&bridge, &package.into(), &url.into());
        return;
    }
    // desktop shell: system opener; plain browser: a normal new tab
    if !open_external(url) {
        let _ = window.open_with_url_and_target(url, "_blank");
    }
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
fn on_android() -> bool {
    web_sys::window()
        .and_then(|w| js_sys::Reflect::get(&w, &"CHAOS_PLATFORM".into()).ok())
        .and_then(|v| v.as_string())
        .is_some_and(|p| p == "android")
}

pub fn use_client() -> ChaosClient {
    let config = use_context::<AppConfig>().expect("AppConfig provided by the shell");
    let token = config.persist_token.then(stored_token).flatten();
    ChaosClient::new(config.api_base).with_token(token)
}

pub fn use_session() -> Session {
    use_context::<Session>().expect("Session provided by App")
}

#[component]
pub fn App(config: AppConfig) -> impl IntoView {
    provide_context(config);

    let session = Session(RwSignal::new(None));
    provide_context(session);

    // Restore the session from the cookie on startup.
    let client = use_client();
    spawn_local(async move {
        if let Ok(user) = client.me().await {
            session.0.set(Some(user));
        }
    });

    // Companion apps: only rendered once the server says they exist.
    let apps = Apps(RwSignal::new(Vec::new()));
    provide_context(apps);
    spawn_local({
        let client = use_client();
        async move {
            if let Ok(list) = client.apps().await {
                apps.0.set(list);
            }
        }
    });

    // Theme: applied as `data-theme` on <body>, persisted, all-CSS.
    // Changed from the settings page via the ThemeSetting context.
    let theme = ThemeSetting(RwSignal::new(stored_theme()));
    provide_context(theme);
    Effect::new(move |_| apply_theme(&theme.0.get()));

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
        if external && open_external(&href) {
            ev.prevent_default();
        }
    });
    on_cleanup(move || external_clicks.remove());

    let logout = Callback::new({
        let client = use_client();
        move |_: leptos::ev::MouseEvent| {
            let client = client.clone();
            spawn_local(async move {
                let _ = client.logout().await;
                store_token(None);
                session.0.set(None);
            });
        }
    });

    view! {
        <ServerGate>
        <Router>
            <ShareRedirect/>
            <nav class="topbar">
                <span class="brand">"chaos"</span>
                <A href="/">"Dashboard"</A>
                <A href="/links">"Links"</A>
                <A href="/calendar">"Calendar"</A>
                {move || {
                    apps.0
                        .get()
                        .into_iter()
                        .map(|app| view! { <AppNavEntry app/> })
                        .collect_view()
                }}
                <span class="topbar-foot">
                    <A href="/settings">"Settings"</A>
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
            <main>
                <Routes fallback=|| view! { <p class="muted">"Page not found"</p> }>
                    <Route path=path!("/") view=pages::Dashboard/>
                    <Route path=path!("/links") view=pages::Links/>
                    <Route path=path!("/calendar") view=pages::CalendarPage/>
                    <Route path=path!("/login") view=pages::Login/>
                    <Route path=path!("/settings") view=pages::Settings/>
                    <Route path=path!("/apps/:id") view=pages::AppPage/>
                </Routes>
            </main>
        </Router>
        </ServerGate>
    }
}

/// Sidebar entry for a companion app. Inside the Android shell (with a
/// configured package) it launches the native app / URL through the
/// bridge; everywhere else it routes to the embedded view.
#[component]
fn AppNavEntry(app: AppLink) -> impl IntoView {
    match (on_android(), app.android_package) {
        (true, Some(package)) => {
            let url = app.url.to_string();
            view! {
                <a
                    href="#"
                    on:click=move |ev| {
                        ev.prevent_default();
                        open_app_native(&package, &url);
                    }
                >
                    {app.title}
                </a>
            }
            .into_any()
        }
        _ => view! { <A href=format!("/apps/{}", app.id)>{app.title}</A> }.into_any(),
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
    spawn_local(async move {
        match client.health().await {
            Ok(_) => gate.set(GateState::Ready),
            Err(_) => gate.set(GateState::Unreachable),
        }
    });

    let current = use_client().base().to_string();
    let input = RwSignal::new(current);
    let connect = move |_| {
        let value = input.get_untracked().trim().to_string();
        if Url::parse(&value).is_err() {
            return;
        }
        if let Some(window) = web_sys::window() {
            if let Ok(Some(storage)) = window.local_storage() {
                let _ = storage.set_item(API_BASE_KEY, &value);
            }
            let _ = window.location().reload();
        }
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
