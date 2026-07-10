//! Home Assistant integration: temperature history and light control.
//!
//! chaos-server proxies Home Assistant server-side (it holds the long-lived
//! access token); these are the wire types exposed to clients. Entity ids
//! and the HA base URL/token stay server-side config, never exposed here.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A configured temperature sensor, as shown in the Home tab's sensor list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HomeSensorInfo {
    pub id: String,
    pub label: String,
    /// Battery level 0–100 of the sensor device, when it exposes one
    /// (auto-derived `*_battery` sibling entity or a configured override).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_pct: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Current state of a configured light.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LightState {
    pub id: String,
    pub label: String,
    /// False when Home Assistant couldn't be reached or the entity doesn't
    /// exist; `on`/`brightness`/`color` are stale/defaulted in that case.
    pub available: bool,
    pub on: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<RgbColor>,
}

/// Partial update sent to `POST /api/v1/home/lights/{id}`. Only set fields
/// are changed; `on: Some(false)` turns the light off regardless of the
/// other fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LightCommand {
    #[serde(default)]
    pub on: Option<bool>,
    /// 0-100.
    #[serde(default)]
    pub brightness: Option<u8>,
    #[serde(default)]
    pub color: Option<RgbColor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TemperatureReading {
    pub at: DateTime<Utc>,
    pub celsius: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemperatureSeries {
    pub id: String,
    pub label: String,
    pub readings: Vec<TemperatureReading>,
}

/// Query parameters of `GET /api/v1/home/temperature`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemperatureQuery {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}
