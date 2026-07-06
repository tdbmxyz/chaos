use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use crate::{use_apps, use_theme};

/// Embedded companion app (web + desktop): the app's own UI in an iframe,
/// with the chaos theme forwarded as a query parameter so the looks stay in
/// sync (the app decides what to do with it — see yomu).
#[component]
pub fn AppPage() -> impl IntoView {
    let params = use_params_map();
    let apps = use_apps();
    let theme = use_theme().0;

    view! {
        {move || {
            let id = params.get().get("id").unwrap_or_default();
            let app = apps.0.get().into_iter().find(|a| a.id == id);
            match app {
                Some(app) => {
                    let sep = if app.url.query().is_some() { '&' } else { '?' };
                    let src = format!("{}{sep}chaos-theme={}", app.url, theme.get());
                    // allowfullscreen: apps drive their own fullscreen (the
                    // yomu reader has a button for it); without the grant
                    // requestFullscreen inside the frame is a silent no-op.
                    view! {
                        <iframe
                            class="app-frame"
                            title=app.title
                            src=src
                            allowfullscreen="true"
                            allow="fullscreen"
                        ></iframe>
                    }
                        .into_any()
                }
                // Apps load right after mount; a direct hit on /apps/x shows
                // this for a frame at most (or truly unknown ids forever).
                None => view! { <p class="muted">"Unknown app."</p> }.into_any(),
            }
        }}
    }
}
