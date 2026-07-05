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
}

/// Who is signed in, if anyone. `None` is a first-class state: everything
/// except calendars works logged off.
#[derive(Clone, Copy)]
pub struct Session(pub RwSignal<Option<User>>);

pub fn use_client() -> ChaosClient {
    let config = use_context::<AppConfig>().expect("AppConfig provided by the shell");
    ChaosClient::new(config.api_base)
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

    let logout = {
        let client = use_client();
        move |_| {
            let client = client.clone();
            spawn_local(async move {
                let _ = client.logout().await;
                session.0.set(None);
            });
        }
    };

    view! {
        <Router>
            <nav class="topbar">
                <span class="brand">"chaos"</span>
                <A href="/">"Dashboard"</A>
                <A href="/links">"Links"</A>
                <A href="/calendar">"Calendar"</A>
                <span class="topbar-account">
                    {move || match session.0.get() {
                        Some(user) => {
                            view! {
                                <span class="topbar-user">{user.display_name}</span>
                                <button class="topbar-logout" title="Sign out" on:click=logout.clone()>
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
    }
}
