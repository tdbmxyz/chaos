//! `/api/v1/home/*`: the Home tab — temperature history and light control,
//! proxied through Home Assistant. Unauthenticated for now, like the
//! services/systemd routes (see `api/mod.rs`).

use axum::Json;
use axum::extract::{Path, Query, State};
use chaos_domain::{HomeSensorInfo, LightCommand, LightState, TemperatureQuery, TemperatureSeries};

use crate::api::ApiError;
use crate::home_assistant::HomeAssistantClient;
use crate::state::AppState;

fn require_home(state: &AppState) -> Result<&HomeAssistantClient, ApiError> {
    state.home.as_deref().ok_or(ApiError::NotFound)
}

pub async fn sensors(State(state): State<AppState>) -> Json<Vec<HomeSensorInfo>> {
    let Some(home) = state.home.as_ref() else {
        return Json(Vec::new());
    };
    let mut sensors = Vec::with_capacity(home.sensors.len());
    for def in &home.sensors {
        sensors.push(HomeSensorInfo {
            id: def.id.clone(),
            label: home.label(def).await,
            battery_pct: home.battery_pct(def).await,
        });
    }
    Json(sensors)
}

pub async fn temperature(
    State(state): State<AppState>,
    Query(query): Query<TemperatureQuery>,
) -> Result<Json<Vec<TemperatureSeries>>, ApiError> {
    let home = require_home(&state)?;
    home.temperature_history(query.start, query.end)
        .await
        .map(Json)
        .map_err(ApiError::BadGateway)
}

pub async fn lights(State(state): State<AppState>) -> Result<Json<Vec<LightState>>, ApiError> {
    let home = require_home(&state)?;
    let mut states = Vec::with_capacity(home.lights.len());
    for def in &home.lights {
        states.push(home.light_state(def).await);
    }
    Ok(Json(states))
}

pub async fn set_light(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(cmd): Json<LightCommand>,
) -> Result<Json<LightState>, ApiError> {
    let home = require_home(&state)?;
    let def = home.find_light(&id).ok_or(ApiError::NotFound)?;
    home.set_light(def, &cmd)
        .await
        .map(Json)
        .map_err(ApiError::BadGateway)
}
