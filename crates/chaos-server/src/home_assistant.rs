//! Home Assistant REST API client (temperature history + light control).
//!
//! chaos-server holds the long-lived access token and proxies every call —
//! the browser never talks to Home Assistant directly. Built once in
//! `AppState::new` when `home_assistant.base_url` is configured.

use std::collections::HashMap;
use std::time::Duration;

use chaos_domain::{LightCommand, LightState, RgbColor, TemperatureReading, TemperatureSeries};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::sync::RwLock;
use url::Url;

use crate::config::{HomeAssistantConfig, HomeEntityDef};

const TIMEOUT: Duration = Duration::from_secs(10);

/// After a light command, HA can report the old state for a moment
/// (Zigbee and friends confirm asynchronously). Poll until the command
/// is observed so clients never receive a stale state.
const CONFIRM_POLLS: usize = 8;
const CONFIRM_INTERVAL: Duration = Duration::from_millis(250);

pub struct HomeAssistantClient {
    http: reqwest::Client,
    base_url: Url,
    token: String,
    pub sensors: Vec<HomeEntityDef>,
    pub lights: Vec<HomeEntityDef>,
    /// Labels resolved from Home Assistant for entities configured without
    /// one (area name, then friendly name), cached per entity for the
    /// process lifetime.
    labels: RwLock<HashMap<String, String>>,
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
            labels: RwLock::new(HashMap::new()),
        }))
    }

    /// Display name of an entity: the configured label, or — resolved from
    /// Home Assistant and cached — its area (room), then its friendly name.
    /// Falls back to the public `id` (uncached, so it retries) when Home
    /// Assistant can't answer.
    pub async fn label(&self, def: &HomeEntityDef) -> String {
        if let Some(label) = &def.label {
            return label.clone();
        }
        if let Some(hit) = self.labels.read().await.get(&def.entity_id) {
            return hit.clone();
        }
        match self.resolve_label(&def.entity_id).await {
            Ok(label) => {
                self.labels
                    .write()
                    .await
                    .insert(def.entity_id.clone(), label.clone());
                label
            }
            Err(reason) => {
                tracing::warn!(
                    entity_id = def.entity_id,
                    reason,
                    "home assistant label resolution failed"
                );
                def.id.clone()
            }
        }
    }

    async fn resolve_label(&self, entity_id: &str) -> Result<String, String> {
        let template = format!(
            "{{{{ area_name('{entity_id}') or state_attr('{entity_id}', 'friendly_name') or '' }}}}"
        );
        let resp = self
            .http
            .post(self.url("api/template"))
            .bearer_auth(&self.token)
            .json(&serde_json::json!({ "template": template }))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("home assistant returned {}", resp.status()));
        }
        let label = resp.text().await.map_err(|e| e.to_string())?;
        let label = label.trim();
        if label.is_empty() {
            return Err("no area or friendly name".into());
        }
        Ok(label.to_string())
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

        let mut series = Vec::with_capacity(self.sensors.len());
        for (def, changes) in self.sensors.iter().zip(raw) {
            series.push(TemperatureSeries {
                id: def.id.clone(),
                label: self.label(def).await,
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
            });
        }
        Ok(series)
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
                    label: self.label(def).await,
                    available: false,
                    on: false,
                    brightness: None,
                    color: None,
                }
            }
        }
    }

    /// Battery percentage of a sensor, when it has a battery entity
    /// (configured or derived). Any failure — no entity, HA unreachable,
    /// "unavailable"/"unknown" state — is `None`, never an error: battery
    /// is decoration on the sensor list, not data worth failing over.
    pub async fn battery_pct(&self, def: &HomeEntityDef) -> Option<f64> {
        let (entity_id, configured) = match &def.battery_entity_id {
            Some(id) => (id.clone(), true),
            None => (derive_battery_entity(&def.entity_id)?, false),
        };
        let url = self.url(&format!("api/states/{entity_id}"));
        match self.get_json::<HaEntityState>(url).await {
            Ok(raw) => match raw.state.parse::<f64>() {
                Ok(pct) => Some(pct),
                Err(_) => {
                    if configured {
                        // A typo'd override should be debuggable; the
                        // derived path is silent because a missing sibling
                        // entity is normal.
                        tracing::warn!(
                            entity_id,
                            sensor = def.id,
                            reason = format!("state {:?} is not a number", raw.state),
                            "configured battery entity unreadable"
                        );
                    }
                    None
                }
            },
            Err(reason) => {
                if configured {
                    tracing::warn!(
                        entity_id,
                        sensor = def.id,
                        reason,
                        "configured battery entity unreadable"
                    );
                }
                None
            }
        }
    }

    async fn fetch_light_state(&self, def: &HomeEntityDef) -> Result<LightState, String> {
        let url = self.url(&format!("api/states/{}", def.entity_id));
        let raw: HaEntityState = self.get_json(url).await?;
        Ok(to_light_state(def, self.label(def).await, &raw))
    }

    pub async fn set_light(
        &self,
        def: &HomeEntityDef,
        cmd: &LightCommand,
    ) -> Result<LightState, String> {
        match cmd.on {
            Some(false) => {
                self.call_service("turn_off", def, None, None).await?;
                self.confirm(def, |state| !state.on).await
            }
            Some(true) => {
                self.call_service("turn_on", def, cmd.brightness, cmd.color)
                    .await?;
                self.confirm(def, |state| state.on).await
            }
            // Adjustments only apply to a lit lamp: HA's turn_on would
            // power the light as a side effect of a brightness/color
            // change, so an off light is left untouched. The pre-check
            // polls (not a single fetch): a just-commanded turn-on may
            // still be confirming when the adjustment arrives.
            None => {
                let state = self.confirm(def, |state| state.on).await?;
                if !state.on {
                    return Ok(state);
                }
                self.call_service("turn_on", def, cmd.brightness, cmd.color)
                    .await?;
                let target = cmd.brightness;
                self.confirm(def, move |state| match target {
                    // brightness_pct → 0-255 → pct roundtrips lossily,
                    // hence the tolerance.
                    Some(pct) => state.brightness.is_some_and(|b| b.abs_diff(pct) <= 2),
                    None => true,
                })
                .await
            }
        }
    }

    /// Poll the entity until `settled` observes the commanded state, up to
    /// 1 + CONFIRM_POLLS fetches spaced CONFIRM_INTERVAL apart; then return
    /// the last state seen — a genuinely failed command still reports the
    /// truth. Individual fetch errors are tolerated (the last good reading
    /// wins); only a fully unreachable HA is an error.
    async fn confirm(
        &self,
        def: &HomeEntityDef,
        settled: impl Fn(&LightState) -> bool,
    ) -> Result<LightState, String> {
        let mut last: Result<LightState, String> = Err("no state observed".into());
        for attempt in 0..=CONFIRM_POLLS {
            if attempt > 0 {
                tokio::time::sleep(CONFIRM_INTERVAL).await;
            }
            match self.fetch_light_state(def).await {
                Ok(state) => {
                    if settled(&state) {
                        return Ok(state);
                    }
                    last = Ok(state);
                }
                Err(reason) => {
                    if last.is_err() {
                        last = Err(reason);
                    }
                }
            }
        }
        last
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

/// Battery sibling of a temperature entity: `..._temperature` →
/// `..._battery`, keeping Home Assistant's numeric dedup suffix
/// (`..._temperature_2` → `..._battery_2`). None when the id doesn't end
/// in the pattern.
fn derive_battery_entity(entity_id: &str) -> Option<String> {
    if let Some(base) = entity_id.strip_suffix("_temperature") {
        return Some(format!("{base}_battery"));
    }
    let (rest, n) = entity_id.rsplit_once('_')?;
    if !n.chars().all(|c| c.is_ascii_digit()) || n.is_empty() {
        return None;
    }
    let base = rest.strip_suffix("_temperature")?;
    Some(format!("{base}_battery_{n}"))
}

fn to_light_state(def: &HomeEntityDef, label: String, raw: &HaEntityState) -> LightState {
    LightState {
        id: def.id.clone(),
        label,
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::extract::Path;
    use chaos_domain::LightCommand;

    use super::*;
    use crate::config::{HomeAssistantConfig, HomeEntityDef};

    fn light_def() -> HomeEntityDef {
        HomeEntityDef {
            id: "lamp".into(),
            label: Some("Lamp".into()),
            entity_id: "light.lamp".into(),
            battery_entity_id: None,
        }
    }

    /// Stub HA: `/api/states/{id}` walks through `states` (sticking on the
    /// last one), `/api/services/light/{service}` always succeeds. Returns
    /// the base URL and a counter of state fetches.
    async fn stub_ha(states: Vec<&'static str>) -> (Url, Arc<AtomicUsize>) {
        let fetches = Arc::new(AtomicUsize::new(0));
        let states = Arc::new(states);
        let app = axum::Router::new()
            .route(
                "/api/states/{id}",
                axum::routing::get({
                    let fetches = fetches.clone();
                    let states = states.clone();
                    move |_: axum::extract::Path<String>| {
                        let n = fetches.fetch_add(1, Ordering::SeqCst);
                        let state = states[n.min(states.len() - 1)];
                        let body = serde_json::json!({ "state": state, "attributes": {} });
                        async move { axum::Json(body) }
                    }
                }),
            )
            .route(
                "/api/services/light/{service}",
                axum::routing::post(|| async { axum::Json(serde_json::json!([])) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding stub ha");
        let addr = listener.local_addr().expect("stub ha addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serving stub ha");
        });
        (
            format!("http://{addr}/").parse().expect("stub ha url"),
            fetches,
        )
    }

    /// Boots a client against `base_url` with the given sensors/lights,
    /// sharing the token-file plumbing every test needs.
    fn client_with(
        base_url: Url,
        sensors: Vec<HomeEntityDef>,
        lights: Vec<HomeEntityDef>,
    ) -> HomeAssistantClient {
        let token = std::env::temp_dir().join(format!(
            "chaos-ha-test-token-{}-{:p}",
            std::process::id(),
            &base_url
        ));
        std::fs::write(&token, "test-token").expect("writing stub token");
        HomeAssistantClient::new(&HomeAssistantConfig {
            base_url: Some(base_url),
            token_file: Some(token),
            sensors,
            lights,
        })
        .expect("building client")
        .expect("client is configured")
    }

    fn client(base_url: Url) -> HomeAssistantClient {
        client_with(base_url, vec![], vec![light_def()])
    }

    /// HA reports `off` for a while after turn_on (async confirmation):
    /// set_light must keep polling and answer with the settled `on`.
    #[tokio::test]
    async fn set_light_waits_for_the_commanded_state() {
        let (url, fetches) = stub_ha(vec!["off", "off", "on"]).await;
        let ha = client(url);

        let state = ha
            .set_light(
                &light_def(),
                &LightCommand {
                    on: Some(true),
                    ..Default::default()
                },
            )
            .await
            .expect("set_light");

        assert!(state.on, "should report the confirmed on state");
        assert!(
            fetches.load(Ordering::SeqCst) >= 3,
            "should have polled past the stale readings"
        );
    }

    /// A light that never confirms: after the poll budget the last observed
    /// (real) state is returned rather than hanging or lying.
    #[tokio::test]
    async fn set_light_reports_the_truth_on_confirmation_timeout() {
        let (url, _fetches) = stub_ha(vec!["off"]).await;
        let ha = client(url);

        let state = ha
            .set_light(
                &light_def(),
                &LightCommand {
                    on: Some(true),
                    ..Default::default()
                },
            )
            .await
            .expect("set_light");

        assert!(!state.on, "timeout must surface HA's actual state");
    }

    /// An adjustment arriving while a turn-on is still confirming must wait
    /// for the light to come up rather than dropping the adjustment (and
    /// telling the client the light is off).
    #[tokio::test]
    async fn adjustment_waits_out_the_turn_on_confirmation() {
        let (url, fetches) = stub_ha(vec!["off", "on"]).await;
        let ha = client(url);

        let state = ha
            .set_light(
                &light_def(),
                &LightCommand {
                    color: Some(chaos_domain::RgbColor {
                        r: 255,
                        g: 200,
                        b: 120,
                    }),
                    ..Default::default()
                },
            )
            .await
            .expect("set_light");

        assert!(state.on, "the adjustment must not report a stale off");
        assert!(fetches.load(Ordering::SeqCst) >= 3);
    }

    #[test]
    fn battery_entity_derived_by_suffix_swap() {
        assert_eq!(
            super::derive_battery_entity("sensor.timmerflotte_temp_hmd_sensor_temperature"),
            Some("sensor.timmerflotte_temp_hmd_sensor_battery".into())
        );
        assert_eq!(
            super::derive_battery_entity("sensor.foo_temperature_2"),
            Some("sensor.foo_battery_2".into())
        );
        assert_eq!(super::derive_battery_entity("sensor.foo_humidity"), None);
        assert_eq!(
            super::derive_battery_entity("sensor.foo_temperature_x2"),
            None
        );
    }

    /// Stub HA serving a single fixed entity id with `{"state": "87"}`;
    /// everything else 404s, matching how HA answers unknown entities.
    async fn stub_ha_single_entity(entity_id: &'static str) -> Url {
        let app = axum::Router::new().route(
            "/api/states/{id}",
            axum::routing::get(move |Path(id): Path<String>| async move {
                if id == entity_id {
                    Ok(axum::Json(
                        serde_json::json!({ "state": "87", "attributes": {} }),
                    ))
                } else {
                    Err(axum::http::StatusCode::NOT_FOUND)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding stub ha");
        let addr = listener.local_addr().expect("stub ha addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serving stub ha");
        });
        format!("http://{addr}/").parse().expect("stub ha url")
    }

    #[tokio::test]
    async fn battery_pct_reads_the_derived_entity_and_tolerates_absence() {
        let url = stub_ha_single_entity("sensor.salon_battery").await;
        let ha = client_with(url, vec![], vec![]);

        let with_battery = HomeEntityDef {
            id: "salon".into(),
            label: Some("Salon".into()),
            entity_id: "sensor.salon_temperature".into(),
            battery_entity_id: None,
        };
        assert_eq!(ha.battery_pct(&with_battery).await, Some(87.0));

        let without = HomeEntityDef {
            id: "cave".into(),
            label: Some("Cave".into()),
            entity_id: "sensor.cave_temperature".into(),
            battery_entity_id: None,
        };
        assert_eq!(ha.battery_pct(&without).await, None);
    }

    /// A sensor whose `entity_id` wouldn't derive a battery sibling still
    /// gets a battery reading when `battery_entity_id` is explicitly
    /// configured: the override wins, derivation isn't required.
    #[tokio::test]
    async fn battery_pct_uses_the_configured_override_when_derivation_would_fail() {
        let url = stub_ha_single_entity("sensor.custom_batt").await;
        let ha = client_with(url, vec![], vec![]);

        let overridden = HomeEntityDef {
            id: "weird".into(),
            label: Some("Weird".into()),
            entity_id: "sensor.weird_name".into(),
            battery_entity_id: Some("sensor.custom_batt".into()),
        };
        assert_eq!(super::derive_battery_entity(&overridden.entity_id), None);
        assert_eq!(ha.battery_pct(&overridden).await, Some(87.0));
    }
}
