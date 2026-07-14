use chaos_domain::LoginRequest;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;

use crate::{use_client, use_session};

#[component]
pub fn Login() -> impl IntoView {
    let client = use_client();
    let session = use_session();
    let navigate = use_navigate();

    let username = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let user = username.get_untracked().trim().to_string();
        let pass = password.get_untracked();
        if user.is_empty() || pass.is_empty() {
            return;
        }
        busy.set(true);
        error.set(None);
        let client = client.clone();
        let navigate = navigate.clone();
        spawn_local(async move {
            match client
                .login(&LoginRequest {
                    username: user,
                    password: pass,
                })
                .await
            {
                Ok(resp) => {
                    if crate::persist_token() {
                        crate::store_token(Some(&resp.token));
                    }
                    // A login is a user switch: the previous user's
                    // last-known-good data (calendar, dashboard, …) must not
                    // survive it — an expired session skips the logout path
                    // that would otherwise have cleared it.
                    crate::offline::cache_clear();
                    crate::offline::cache_put("me", &resp.user);
                    session.0.set(Some(resp.user));
                    navigate("/", Default::default());
                }
                Err(err) if err.is_unauthorized() => {
                    error.set(Some("Wrong username or password".into()));
                }
                Err(err) => error.set(Some(err.to_string())),
            }
            busy.set(false);
        });
    };

    view! {
        <div class="login-page">
            <form class="login-card" on:submit=submit>
                <h2>"Sign in"</h2>
                <label>
                    "Username"
                    <input
                        type="text"
                        autocomplete="username"
                        autofocus
                        prop:value=username
                        on:input=move |ev| username.set(event_target_value(&ev))
                    />
                </label>
                <label>
                    "Password"
                    <input
                        type="password"
                        autocomplete="current-password"
                        prop:value=password
                        on:input=move |ev| password.set(event_target_value(&ev))
                    />
                </label>
                {move || error.get().map(|err| view! { <p class="error">{err}</p> })}
                <button type="submit" class="primary" disabled=move || busy.get()>
                    {move || if busy.get() { "Signing in…" } else { "Sign in" }}
                </button>
                <p class="muted login-hint">
                    "Accounts are created on the server: chaos-admin add-user <name>"
                </p>
            </form>
        </div>
    }
}
