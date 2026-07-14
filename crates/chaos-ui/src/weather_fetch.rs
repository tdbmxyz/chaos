//! Client-side weather: direct Open-Meteo with per-place localStorage
//! caches. Weather never touches the chaos server, so it neither depends
//! on nor affects the Connectivity signal — its own failure handling is
//! TTL + serve-stale.

use chaos_client::open_meteo::{self, Place, PlaceForecast};
use chaos_domain::WeatherData;
use serde::{Deserialize, Serialize};

/// Forecasts refresh at most every 10 minutes (matches the TTL the server
/// cache used).
const TTL_MS: f64 = 600.0 * 1000.0;

#[derive(Serialize, Deserialize)]
struct CachedForecast {
    /// js Date.now() epoch millis at fetch time.
    fetched_at_ms: f64,
    forecast: PlaceForecast,
}

fn is_fresh(fetched_at_ms: f64, now_ms: f64) -> bool {
    now_ms - fetched_at_ms < TTL_MS
}

/// One shared upstream HTTP client (reqwest clients are Arcs inside).
pub(crate) fn http() -> reqwest::Client {
    thread_local! {
        static HTTP: reqwest::Client = reqwest::Client::new();
    }
    HTTP.with(Clone::clone)
}

/// Geocode with a permanent per-name cache (a city's coordinates don't
/// move; the resolved display name is worth keeping offline).
async fn place(location: &str) -> Result<Place, String> {
    let key = format!("geocode:{}", location.trim().to_lowercase());
    if let Some(hit) = crate::offline::cache_get::<Place>(&key) {
        return Ok(hit);
    }
    let place = open_meteo::geocode(&http(), location).await?;
    crate::offline::cache_put(&key, &place);
    Ok(place)
}

/// The one weather read path: fresh cache → cached copy; otherwise fetch
/// and overwrite; on fetch failure serve the stale copy if there is one.
/// `now_index` is recomputed on EVERY cached read so a forecast fetched
/// hours ago still points at the current hour.
pub(crate) async fn place_weather(location: &str) -> Result<WeatherData, String> {
    let place = place(location).await?;
    let key = format!("weather:{}", place.name.to_lowercase());
    let now_ms = js_sys::Date::now();

    let cached = crate::offline::cache_get::<CachedForecast>(&key);
    if let Some(hit) = &cached
        && is_fresh(hit.fetched_at_ms, now_ms)
    {
        return Ok(revalidated(hit.forecast.clone()));
    }
    match open_meteo::forecast(&http(), &place).await {
        Ok(forecast) => {
            crate::offline::cache_put(
                &key,
                &CachedForecast {
                    fetched_at_ms: now_ms,
                    forecast: forecast.clone(),
                },
            );
            Ok(forecast.data)
        }
        Err(err) => match cached {
            Some(hit) => Ok(revalidated(hit.forecast)),
            None => Err(err),
        },
    }
}

/// A cached forecast with `now_index` moved to the current local hour.
fn revalidated(forecast: PlaceForecast) -> WeatherData {
    let mut data = forecast.data;
    data.now_index = open_meteo::recompute_now_index(
        &data.hourly,
        chrono::Utc::now(),
        forecast.utc_offset_seconds,
    );
    data
}

/// The place to show when none is given: the device preference, else the
/// location of the dashboard's weather widget (from the cached layout, so
/// it also resolves offline).
pub(crate) async fn default_location() -> Option<String> {
    if let Some(pref) = crate::pref(crate::WEATHER_LOCATION_KEY) {
        return Some(pref);
    }
    let layout = crate::offline::cache_get::<chaos_domain::DashboardLayout>("dashboard")?;
    layout
        .columns
        .iter()
        .flat_map(|c| &c.widgets)
        .find_map(|w| match &w.widget {
            chaos_domain::Widget::Weather { location } => Some(location.clone()),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_is_fresh_within_ttl_and_stale_after() {
        assert!(is_fresh(1_000_000.0, 1_000_000.0 + 599_000.0));
        assert!(!is_fresh(1_000_000.0, 1_000_000.0 + 601_000.0));
    }
}
