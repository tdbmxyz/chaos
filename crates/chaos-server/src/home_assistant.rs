//! Home Assistant REST API client (temperature history + light control).
//!
//! chaos-server holds the long-lived access token and proxies every call —
//! the browser never talks to Home Assistant directly. Built once in
//! `AppState::new` when `home_assistant.base_url` is configured.

use std::time::Duration;

use chaos_domain::{LightCommand, LightState, RgbColor, TemperatureReading, TemperatureSeries};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use url::Url;

use crate::config::{HomeAssistantConfig, HomeEntityDef};

const TIMEOUT: Duration = Duration::from_secs(10);

pub struct HomeAssistantClient {
    http: reqwest::Client,
    base_url: Url,
    token: String,
    pub sensors: Vec<HomeEntityDef>,
    pub lights: Vec<HomeEntityDef>,
}

impl HomeAssistantClient {
    /// `None` when the integration isn't configured (`base_url` unset).
    pub fn new(config: &HomeAssistantConfig) -> anyhow::Result<Option<Self>> {
        let Some(base_url) = config.base_url.clone() else {
            return Ok(None);
        };
        let token_file = config.token_file.as_ref().ok_or_else(|| {
            anyhow::anyhow!("home_assistant.base_url is set but token_file is not")
        })?;
        let token = std::fs::read_to_string(token_file)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", token_file.display()))?
            .trim()
            .to_string();

        Ok(Some(Self {
            http: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .build()
                .expect("building home assistant http client"),
            base_url,
            token,
            sensors: config.sensors.clone(),
            lights: config.lights.clone(),
        }))
    }

    fn url(&self, path: &str) -> Url {
        self.base_url
            .join(path)
            .unwrap_or_else(|_| panic!("invalid home assistant path {path:?}"))
    }

    pub fn find_light(&self, id: &str) -> Option<&HomeEntityDef> {
        self.lights.iter().find(|l| l.id == id)
    }

    /// History for the given sensors between `start` and `end`. Entities that
    /// never reported in the window come back with an empty reading list.
    pub async fn temperature_history(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<TemperatureSeries>, String> {
        if self.sensors.is_empty() {
            return Ok(Vec::new());
        }
        let entity_ids: Vec<&str> = self.sensors.iter().map(|s| s.entity_id.as_str()).collect();
        let mut url = self.url(&format!("api/history/period/{}", start.to_rfc3339()));
        url.query_pairs_mut()
            .append_pair("filter_entity_id", &entity_ids.join(","))
            .append_pair("end_time", &end.to_rfc3339())
            .append_pair("minimal_response", "");

        // One array per requested entity, in request order (per HA's docs).
        // `minimal_response` drops `entity_id` (and attributes) from every
        // entry but the first for a given entity, so match by array
        // position instead of by `entity_id`.
        let mut raw: Vec<Vec<HaStateChange>> = self.get_json(url).await?;
        raw.resize_with(self.sensors.len(), Vec::new);

        Ok(self
            .sensors
            .iter()
            .zip(raw)
            .map(|(def, changes)| TemperatureSeries {
                id: def.id.clone(),
                label: def.label.clone(),
                readings: changes
                    .into_iter()
                    .filter_map(|change| {
                        // "unavailable"/"unknown" don't parse as a number.
                        let celsius = change.state.parse::<f64>().ok()?;
                        Some(TemperatureReading {
                            at: change.last_changed,
                            celsius,
                        })
                    })
                    .collect(),
            })
            .collect())
    }

    pub async fn light_state(&self, def: &HomeEntityDef) -> LightState {
        match self.fetch_light_state(def).await {
            Ok(state) => state,
            Err(reason) => {
                tracing::warn!(
                    entity_id = def.entity_id,
                    reason,
                    "home assistant light state fetch failed"
                );
                LightState {
                    id: def.id.clone(),
                    label: def.label.clone(),
                    available: false,
                    on: false,
                    brightness: None,
                    color: None,
                }
            }
        }
    }

    async fn fetch_light_state(&self, def: &HomeEntityDef) -> Result<LightState, String> {
        let url = self.url(&format!("api/states/{}", def.entity_id));
        let raw: HaEntityState = self.get_json(url).await?;
        Ok(to_light_state(def, &raw))
    }

    pub async fn set_light(
        &self,
        def: &HomeEntityDef,
        cmd: &LightCommand,
    ) -> Result<LightState, String> {
        if cmd.on == Some(false) {
            self.call_service("turn_off", def, None, None).await?;
        } else {
            self.call_service("turn_on", def, cmd.brightness, cmd.color)
                .await?;
        }
        self.fetch_light_state(def).await
    }

    async fn call_service(
        &self,
        service: &str,
        def: &HomeEntityDef,
        brightness: Option<u8>,
        color: Option<RgbColor>,
    ) -> Result<(), String> {
        let mut body = serde_json::json!({ "entity_id": def.entity_id });
        if let Some(pct) = brightness {
            body["brightness_pct"] = serde_json::json!(pct);
        }
        if let Some(c) = color {
            body["rgb_color"] = serde_json::json!([c.r, c.g, c.b]);
        }

        let url = self.url(&format!("api/services/light/{service}"));
        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("home assistant returned {}", resp.status()));
        }
        Ok(())
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: Url) -> Result<T, String> {
        let resp = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("home assistant returned {}", resp.status()));
        }
        resp.json::<T>().await.map_err(|e| e.to_string())
    }
}

fn to_light_state(def: &HomeEntityDef, raw: &HaEntityState) -> LightState {
    LightState {
        id: def.id.clone(),
        label: def.label.clone(),
        available: raw.state != "unavailable",
        on: raw.state == "on",
        brightness: raw
            .attributes
            .brightness
            .map(|b| ((b as u32 * 100) / 255) as u8),
        color: raw
            .attributes
            .rgb_color
            .map(|[r, g, b]| RgbColor { r, g, b }),
    }
}

#[derive(Debug, Deserialize)]
struct HaStateChange {
    state: String,
    last_changed: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct HaEntityState {
    state: String,
    #[serde(default)]
    attributes: HaLightAttributes,
}

#[derive(Debug, Default, Deserialize)]
struct HaLightAttributes {
    brightness: Option<u8>,
    rgb_color: Option<[u8; 3]>,
}
