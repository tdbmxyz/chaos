mod api;
mod archiver;
mod auth;
mod cache;
mod config;
mod db;
mod db_auth;
mod db_calendar;
mod home_assistant;
mod http_util;
mod ics;
mod import;
mod metadata;
mod monitor;
mod notify;
mod state;
mod widgets;

use anyhow::Context;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = config::load().context("loading configuration")?;
    tracing::info!(services = config.services.len(), "configuration loaded");

    let db = db::Db::connect(&config.db_path)
        .await
        .with_context(|| format!("opening database {}", config.db_path.display()))?;

    let state = state::AppState::new(config, db).context("initializing application state")?;

    // One-shot CLI modes.
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => {}
        [cmd, path, owner] if cmd == "import-linkwarden" => {
            return import::linkwarden(&state, std::path::Path::new(path), Some(owner)).await;
        }
        [cmd, path] if cmd == "import-linkwarden" => {
            return import::linkwarden(&state, std::path::Path::new(path), None).await;
        }
        [cmd, rest @ ..] if cmd == "add-user" && !rest.is_empty() => {
            let username = &rest[0];
            let display_name = rest.get(1).cloned().unwrap_or_else(|| username.clone());
            return add_user(&state, username, &display_name).await;
        }
        [cmd] if cmd == "list-users" => {
            for user in state.db.list_users().await? {
                println!("{}  {} ({})", user.id, user.username, user.display_name);
            }
            return Ok(());
        }
        _ => anyhow::bail!(
            "unknown arguments {args:?}; usage: chaos-server \
             [import-linkwarden <file> [owner-username] | add-user <username> [display name] | list-users]"
        ),
    }

    monitor::spawn(state.clone());
    archiver::spawn(state.clone());
    if state.notifier.is_some() && state.config.notifications.calendar_reminders {
        notify::spawn_reminders(state.clone());
    }

    let app = api::router(state.clone());
    let listener = tokio::net::TcpListener::bind(state.config.listen)
        .await
        .with_context(|| format!("binding {}", state.config.listen))?;
    tracing::info!("listening on http://{}", state.config.listen);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn add_user(
    state: &state::AppState,
    username: &str,
    display_name: &str,
) -> anyhow::Result<()> {
    use std::io::{BufRead, IsTerminal};

    // Interactive: hidden prompt with confirmation. Piped (scripts, tests):
    // one password per line on stdin.
    let (password, confirm) = if std::io::stdin().is_terminal() {
        (
            rpassword::prompt_password("Password: ").context("reading password")?,
            rpassword::prompt_password("Confirm password: ").context("reading password")?,
        )
    } else {
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .context("reading password from stdin")?;
        let pass = line.trim_end_matches(['\r', '\n']).to_string();
        (pass.clone(), pass)
    };
    anyhow::ensure!(password == confirm, "passwords do not match");
    anyhow::ensure!(
        password.len() >= 8,
        "password must be at least 8 characters"
    );

    let hash = auth::hash_password(&password)?;
    let user = state.db.create_user(username, display_name, &hash).await?;
    println!("created user {} ({})", user.username, user.id);
    Ok(())
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl-c");
    tracing::info!("shutting down");
}
