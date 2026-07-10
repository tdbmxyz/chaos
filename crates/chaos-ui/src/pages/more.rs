//! The mobile "More" tab: destinations that don't fit the five-slot bottom
//! bar (Calendar, Settings, About) plus the account block. On desktop the
//! sidebar shows everything, so this page is only reachable on phones.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;

use crate::{store_token, use_client, use_session};

#[component]
pub fn MorePage() -> impl IntoView {
    let session = use_session();
    let client = use_client();
    let logout = Callback::new(move |_: leptos::ev::MouseEvent| {
        let client = client.clone();
        spawn_local(async move {
            let _ = client.logout().await;
            store_token(None);
            session.0.set(None);
        });
    });

    view! {
        <div class="more-page">
            <h2>"More"</h2>
            <nav class="more-list">
                <A href="/calendar"><span class="nav-icon">"▣"</span>"Calendar"</A>
                <A href="/settings"><span class="nav-icon">"⚙"</span>"Settings"</A>
                <A href="/about"><span class="nav-icon">"ⓘ"</span>"About"</A>
            </nav>
            <div class="more-account">
                {move || match session.0.get() {
                    Some(user) => view! {
                        <span class="topbar-user">{user.display_name}</span>
                        <button class="topbar-logout" on:click=move |ev| logout.run(ev)>"Sign out"</button>
                    }
                        .into_any(),
                    None => view! { <A href="/login">"Sign in"</A> }.into_any(),
                }}
            </div>
        </div>
    }
}
