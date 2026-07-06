//! Background health monitor: polls each configured service and stores the
//! result in `AppState::statuses` (the glance "monitor" widget equivalent).
//!
//! On-demand services (those with a `unit`) are asked to systemd first: a
//! deliberately stopped unit is `Paused`, not `Down`, and skips the HTTP
//! probe entirely.

use std::time::{Duration, Instant};

use chaos_domain::{HealthState, ServiceDef, ServiceStatus};
use chrono::Utc;

use crate::config::MonitorConfig;
use crate::state::AppState;
use crate::widgets::systemd;

pub fn spawn(state: AppState) {
    tokio::spawn(run(state));
}

/// The HTTP client used for health probes. Also built one-off by the
/// service-action handler to re-check right after a start/stop.
pub fn client(config: &MonitorConfig) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_millis(config.timeout_ms))
        // Self-hosted services often use self-signed certificates on the LAN.
        .danger_accept_invalid_certs(true)
        .build()
        .expect("building monitor http client")
}

async fn run(state: AppState) {
    let interval = Duration::from_secs(state.config.monitor.interval_secs);
    let client = client(&state.config.monitor);

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

pub async fn check(client: &reqwest::Client, service: &ServiceDef) -> ServiceStatus {
    let Some(unit) = &service.unit else {
        return probe_http(client, service).await;
    };

    match systemd::query(unit).await {
        Ok((active_state, _)) => match active_state.as_str() {
            "active" => {
                let mut status = probe_http(client, service).await;
                // systemd reports active before slow apps bind their port
                // (JVM services take a while); a running unit with a dead
                // port reads as still starting, not down.
                if status.state == HealthState::Down {
                    status.state = HealthState::Starting;
                }
                status
            }
            "activating" | "reloading" => status_only(HealthState::Starting, None),
            "failed" => status_only(HealthState::Down, Some(format!("unit {unit} failed"))),
            // inactive / deactivating: stopped on purpose — the default
            // state for on-demand services, so no HTTP check and no alarm.
            "inactive" | "deactivating" => status_only(HealthState::Paused, None),
            other => status_only(
                HealthState::Unknown,
                Some(format!("unit {unit} is {other}")),
            ),
        },
        Err(reason) => {
            tracing::warn!(unit, reason, "systemd query failed for service");
            status_only(HealthState::Unknown, Some(reason))
        }
    }
}

fn status_only(state: HealthState, error: Option<String>) -> ServiceStatus {
    ServiceStatus {
        state,
        http_status: None,
        latency_ms: None,
        checked_at: Some(Utc::now()),
        error,
    }
}

async fn probe_http(client: &reqwest::Client, service: &ServiceDef) -> ServiceStatus {
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
