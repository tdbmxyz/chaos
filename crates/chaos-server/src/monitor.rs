//! Background health monitor: polls each configured service and stores the
//! result in `AppState::statuses` (the glance "monitor" widget equivalent).

use std::time::{Duration, Instant};

use chaos_domain::{HealthState, ServiceDef, ServiceStatus};
use chrono::Utc;

use crate::state::AppState;

pub fn spawn(state: AppState) {
    tokio::spawn(run(state));
}

async fn run(state: AppState) {
    let timeout = Duration::from_millis(state.config.monitor.timeout_ms);
    let interval = Duration::from_secs(state.config.monitor.interval_secs);

    let client = reqwest::Client::builder()
        .timeout(timeout)
        // Self-hosted services often use self-signed certificates on the LAN.
        .danger_accept_invalid_certs(true)
        .build()
        .expect("building monitor http client");

    loop {
        let sweep_started = Instant::now();

        let checks = state
            .config
            .services
            .iter()
            .map(|service| check(&client, service));
        let results = futures::future::join_all(checks).await;

        {
            let mut statuses = state.statuses.write().await;
            for (service, status) in state.config.services.iter().zip(results) {
                statuses.insert(service.id.clone(), status);
            }
        }
        tracing::debug!(
            elapsed_ms = sweep_started.elapsed().as_millis() as u64,
            "monitor sweep done"
        );

        tokio::time::sleep(interval).await;
    }
}

async fn check(client: &reqwest::Client, service: &ServiceDef) -> ServiceStatus {
    let started = Instant::now();
    let result = client
        .get(service.effective_check_url().clone())
        .send()
        .await;
    let latency_ms = started.elapsed().as_millis() as u64;

    match result {
        Ok(resp) => {
            let code = resp.status();
            // Anything below 500 proves the service is alive: auth walls
            // (401/403) and redirects are healthy answers for a dashboard.
            let state = if code.is_server_error() {
                HealthState::Degraded
            } else {
                HealthState::Up
            };
            ServiceStatus {
                state,
                http_status: Some(code.as_u16()),
                latency_ms: Some(latency_ms),
                checked_at: Some(Utc::now()),
                error: None,
            }
        }
        Err(err) => ServiceStatus {
            state: HealthState::Down,
            http_status: None,
            latency_ms: None,
            checked_at: Some(Utc::now()),
            error: Some(err.to_string()),
        },
    }
}
