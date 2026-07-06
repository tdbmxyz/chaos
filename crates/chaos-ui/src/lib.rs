//! Shared Leptos UI: the same `App` component is mounted by the web bundle
//! (trunk) and rendered inside the Tauri webview. Anything platform-specific
//! (like where the API lives) is injected from the outside via [`AppConfig`].

mod components;
mod pages;

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
const LAYOUT_KEY: &str = "chaos-layout";

/// (id, label) of every selectable palette (colors/typography only —
/// structure is the orthogonal [`LAYOUTS`] setting). Pure CSS under
/// `body[data-theme="…"]`; the first one is the default.
pub const THEMES: &[(&str, &str)] = &[
    ("midnight", "Midnight"),
    ("daylight", "Daylight"),
    ("glass", "Glass"),
    ("terminal", "Terminal"),
];

/// (id, label) of every structural layout — navigation position, widget
/// arrangement, density — under `body[data-layout="…"]`. Combines freely
/// with any palette.
pub const LAYOUTS: &[(&str, &str)] = &[
    ("columns", "Columns"),
    ("sidebar", "Sidebar"),
    ("hub", "Hub"),
    ("bento", "Bento"),
];

fn stored_setting(key: &str, options: &[(&str, &str)]) -> String {
    local_storage()
        .and_then(|s| s.get_item(key).ok().flatten())
        .filter(|v| options.iter().any(|(id, _)| id == v))
        .unwrap_or_else(|| options[0].0.to_string())
}

fn apply_setting(attr: &str, key: &str, value: &str) {
    if let Some(body) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.body())
    {
        let _ = body.set_attribute(attr, value);
    }
    if let Some(storage) = local_storage() {
        let _ = storage.set_item(key, value);
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

/// Who is signed in, if anyone. `None` is a first-class state: everything
/// except calendars works logged off.
#[derive(Clone, Copy)]
pub struct Session(pub RwSignal<Option<User>>);

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

    // Palette + structural layout: applied as `data-theme` / `data-layout`
    // on <body>, persisted, all-CSS.
    let theme = RwSignal::new(stored_setting(THEME_KEY, THEMES));
    Effect::new(move |_| apply_setting("data-theme", THEME_KEY, &theme.get()));
    let layout = RwSignal::new(stored_setting(LAYOUT_KEY, LAYOUTS));
    Effect::new(move |_| apply_setting("data-layout", LAYOUT_KEY, &layout.get()));

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
                <span class="pickers">
                    <SettingPicker signal=layout options=LAYOUTS title="Layout"/>
                    <SettingPicker signal=theme options=THEMES title="Theme"/>
                </span>
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
            </nav>
            <main>
                <Routes fallback=|| view! { <p class="muted">"Page not found"</p> }>
                    <Route path=path!("/") view=pages::Dashboard/>
                    <Route path=path!("/links") view=pages::Links/>
                    <Route path=path!("/calendar") view=pages::CalendarPage/>
                    <Route path=path!("/login") view=pages::Login/>
                </Routes>
            </main>
        </Router>
        </ServerGate>
    }
}

/// One topbar dropdown for a persisted look setting (layout or palette).
#[component]
fn SettingPicker(
    signal: RwSignal<String>,
    options: &'static [(&'static str, &'static str)],
    title: &'static str,
) -> impl IntoView {
    view! {
        <select
            class="theme-picker"
            title=title
            on:change=move |ev| signal.set(event_target_value(&ev))
        >
            {options
                .iter()
                .map(|(id, label)| {
                    let id = *id;
                    view! {
                        <option value=id selected=move || signal.get() == id>
                            {*label}
                        </option>
                    }
                })
                .collect_view()}
        </select>
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
