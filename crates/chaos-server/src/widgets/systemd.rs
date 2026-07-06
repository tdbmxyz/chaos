//! Systemd unit status and control (the "services manager" widget).
//!
//! Shells out to `systemctl`, which authorizes non-root callers through
//! polkit — on NixOS the `services.chaos.systemdControl` module option
//! installs a rule scoped to exactly the configured units. Only units
//! listed in the widget definition are ever passed to systemctl, so the
//! attack surface is the config file, not the HTTP API.

use std::time::Duration;

use chaos_domain::{SystemdAction, SystemdUnitDef, SystemdUnitStatus, WidgetData};
use tokio::process::Command;

/// Starting a slow service can legitimately take a while; systemctl itself
/// waits for the job to complete.
const ACTION_TIMEOUT: Duration = Duration::from_secs(60);
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn fetch(units: &[SystemdUnitDef]) -> Result<WidgetData, String> {
    let statuses = futures::future::join_all(units.iter().map(status)).await;
    Ok(WidgetData::Systemd { units: statuses })
}

async fn status(def: &SystemdUnitDef) -> SystemdUnitStatus {
    let title = def.title.clone().unwrap_or_else(|| def.unit.clone());
    let (active_state, sub_state) = match query(&def.unit).await {
        Ok(states) => states,
        Err(reason) => {
            tracing::warn!(unit = def.unit, reason, "systemctl status query failed");
            ("unknown".into(), String::new())
        }
    };
    SystemdUnitStatus {
        unit: def.unit.clone(),
        title,
        active_state,
        sub_state,
        controllable: def.controllable,
    }
}

/// `systemctl show` exits 0 even for unknown units (LoadState=not-found),
/// so the load state doubles as the "does this unit exist" signal. Also
/// used by the service monitor for on-demand services (`ServiceDef::unit`).
pub async fn query(unit: &str) -> Result<(String, String), String> {
    let output = tokio::time::timeout(
        STATUS_TIMEOUT,
        Command::new("systemctl")
            .args([
                "show",
                "--property=LoadState,ActiveState,SubState",
                "--",
                unit,
            ])
            .output(),
    )
    .await
    .map_err(|_| "systemctl show timed out".to_string())?
    .map_err(|e| e.to_string())?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let prop = |key: &str| {
        stdout
            .lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .unwrap_or("")
            .to_string()
    };
    if prop("LoadState") == "not-found" {
        return Ok(("not-found".into(), String::new()));
    }
    Ok((prop("ActiveState"), prop("SubState")))
}

/// Run a start/stop/restart. The caller (WidgetHub) has already checked the
/// unit against the widget's configured allowlist.
pub async fn act(unit: &str, action: SystemdAction) -> Result<(), String> {
    let output = tokio::time::timeout(
        ACTION_TIMEOUT,
        Command::new("systemctl")
            .args([action.verb(), "--", unit])
            .output(),
    )
    .await
    .map_err(|_| format!("systemctl {} timed out", action.verb()))?
    .map_err(|e| e.to_string())?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("systemctl {} failed", action.verb())
        } else {
            stderr
        });
    }
    Ok(())
}
