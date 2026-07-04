mod api;
mod config;
mod db;
mod metadata;
mod monitor;
mod state;

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

    let state = state::AppState::new(config, db);
    monitor::spawn(state.clone());

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

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl-c");
    tracing::info!("shutting down");
}
